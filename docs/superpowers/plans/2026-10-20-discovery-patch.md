# Headstate 10.9.4 Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Finish #1921 and #1922 and publish a verified desktop patch.
**Architecture:** Persist explicit discovery exclusions in SQLite settings; apply them before traversal and budget accounting. Exclusions win even over explicit roots, but do not filter Git-registered worktrees or disk/artifact accounting. Key persisted worktree snapshots by both roots and exclusions; cancel and reset affected UI queries after changes.
**Tech Stack:** Rust, SQLite, React, TanStack Query, Tauri.
**Spec:** GitHub issues #1921 and #1922, authorized by the user's patch-release request.

## Global Constraints
- Use ~/code/ghstat; do not create another checkout.
- Preserve update safety checks and synthetic data; no filesystem cleanup.
- Do not publish private paths or logs.
- Require all CI attempts successful before tagging; verify published assets.

## Review Focus
- Exclusions use component containment, not string-prefix matching.
- Missing exclusions remain effective if recreated; relative paths are rejected.
- Changed exclusions cannot resurrect a stale in-flight persisted snapshot.
- Explicit roots under exclusions remain excluded; siblings and registered worktrees remain discoverable.
- Artifacts, size accounting, and deletion authorization remain independent.

### Task 1: Discovery policy and regression tests
- [x] Add failing deep-backup/sibling/explicit-root exclusion tests in worktrees/scan.rs.
- [x] Add normalized exclusion policy and apply before limits, preserving existing default scanner.
- [x] Clarify diagnostics: skipped subtree count is not depth.
- [x] Verify scanner tests and commit.

### Task 2: Persistence, settings, and cache consistency
- [x] Add settings get/set commands, both surface classifications, dispatch, and frontend API mappings.
- [x] Key worktree persisted snapshots by captured roots plus exclusions; use same policy for bulk update.
- [x] Add exclusion editor under Settings > Repositories; reset/cancel worktree queries when exclusions change.
- [x] Test persistence, cache keys, validation, frontend save and query invalidation.
- [x] Regenerate wire contract, verify focused tests, and commit.

### Task 3: Verify and publish
- [x] #1922 fix committed with RED/GREEN test and full Rust verification.
- [x] Run full Rust/UI/mobile tests and make lint.
- [x] Independent branch review and address substantive findings.
- [ ] Open PR closing both issues, pass CI, merge and verify main content.
- [ ] Verify every check attempt; tag v10.9.4, publish, verify artifact versions/signatures/notarization.
- [ ] Update both issues with release evidence and confirm closed after merge.

## Verification notes
- #1922: original scope regression failed before the main-checkout filter; fixed in 1bd0f3a.
- #1921: excluded-backup traversal and missing settings control reproduced before implementation.
- Independent review found missing-alias matching and unloaded-settings overwrite risks. Both received failing regressions and fixes; policy-aware cache retention was also corrected. No findings deferred.
- Final Rust run: 3,330 library tests passed, 65 ignored, plus integration suites. Frontend: 5,044 passed, 1 skipped. Mobile: 242 core tests plus 43 plugin tests passed.
