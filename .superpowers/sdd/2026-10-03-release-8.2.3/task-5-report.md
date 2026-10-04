# Task 5 report — truthful Ready availability and qualified chronology

Implemented #1667/#1672 without changing readiness enum meaning, action preflight, GitLab behavior, or Task 4 detail fingerprints.

## Result

- `ReadyStrip` receives inventory status plus accepted-receipt coverage from `App`. Empty states now distinguish cold pending, no-receipt failure, complete empty, partial empty, and legacy/unknown coverage. Accepted rows remain visible during a held or failed newer refresh.
- `RowObservation.ready_at_state` is an optional/defaulted `observed | retained` field, separate from readiness/detail fields. Old snapshots and old desktop/mobile payloads decode with unknown date evidence; GitLab leaves it absent.
- GitHub marks structurally valid non-draft timeline evidence observed. A missing/refused/malformed timeline is unknown and never invents a timestamp.
- Reconciliation carries a known timestamp only across the same identity, nonempty same head and base, positively observed head/base/draft facts, and two observed non-draft rows. Changed head/base, draft facts, first-seen unknown, and old unqualified timestamps do not inherit. A positive newer event supersedes retained evidence.
- Retained dates preserve Ready ordering and show the existing compact `Last known` row label plus qualified age title/screen-reader text.

## Meaningful RED evidence

- Narrow mutation restoring the old unconditional `Nothing ready to review.` branch: `VITE_TARGET=desktop yarn vitest run src/App.test.tsx -t 'Ready inventory availability' --reporter=dot` failed 4/7 cases (cold pending, no-receipt failure, partial empty, unknown empty); 3/7 passed. Mutation was immediately reverted.
- Narrow mutation disabling `retain_ready_at`: `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/headstate-pr1596-review/src-tauri/target cargo test --manifest-path src-tauri/Cargo.toml --lib inventory::tests::ready_date_retains_only_across_positive_compatible_facts --no-fail-fast` failed 0/1 at the expected retained timestamp assertion (`None` versus `2026-08-19T15:30:00Z`). Mutation was immediately reverted.
- An earlier 178-test frontend pass had 174 pass/4 fail because the mounted App fixture lacked Task 4 advisory display/worktree fields. Those were harness failures, not behavioral RED evidence; the fixture was corrected and the failure is not claimed as product proof.

## GREEN verification

- `VITE_TARGET=desktop yarn vitest run src/App.test.tsx src/components/ReadyStrip.test.tsx src/lib/derive.test.ts --reporter=dot`: 179 passed, 0 failed.
- Focused availability rerun: 7 passed, 31 filtered.
- `cargo test ... --lib inventory::`: 11 passed, 0 failed (before the final focused mutation/green; final new regression separately passed 1/1).
- `cargo test ... --lib github::map::tests::`: 62 passed, 0 failed.
- GitLab parity and old-cache defaults: 1/1 each passed.
- `make check-wire-contract`: 12 Node contract tests passed and generated schema fresh.
- `yarn tsc --noEmit`: passed.
- `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/headstate-pr1596-review/src-mobile/target make test-mobile`: 224 tests passed across the companion and plugin crates (181 + 16 + 13 + 14), 0 failed. The sandboxed first attempt had 137 pass/44 loopback-bind failures at `src/testing.rs:243` (`Operation not permitted`); the authorized unsandboxed rerun passed.
- `CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/headstate-pr1596-review/src-tauri/target make lint`: passed after an authorized rerun allowed its `ps`-based guard. It reports the repository's existing 34 ESLint warnings, 3 knip hints, and advisory GitHub cache-budget warning; 0 lint errors.
- `cargo fmt --manifest-path src-tauri/Cargo.toml`, `git diff --check`: passed.

One attempted Cargo command supplied two test filters and was rejected by Cargo's CLI; the tests were rerun separately and passed.

## Interface handoff and limits

`ReadyStrip` accepts `availability: { status: "pending" | "failed" | "available"; coverage: SourceCoverage | null }`. `App` is the only production caller and passes it explicitly. The optional complete default exists for older isolated callers/tests; accepted source coverage remains the sole completion proof in production.

`RowObservation.ready_at_state?: "observed" | "retained"` is additive and omitted when unknown. Task 4's source fingerprint remains unchanged and ignores it.

A draft-to-ready-to-draft cycle entirely hidden between polls cannot be proven. Same head alone does not certify lifecycle continuity; the retained date is visibly last-known until a positive timeline event supersedes it. Browser/native-provider integration belongs to Task 7/controller evidence.
