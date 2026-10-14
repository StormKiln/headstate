# HeadState 10.2 disk reliability implementation

Approved specification: epic #1793 and child issues #1794–#1805.

Goal: explain disk use across System Health, Artifacts and Worktrees, including nested workspaces and opted-in external caches. Preserve useful partial measurements and trustworthy growth. No automatic deletion; preserve source, synthetic screenshot data and release evidence.

Architecture: bounded native discovery feeds exclusive allocation accounting and persisted observations. Legacy cleanup catalogs remain ownership-checked logical-size views. React exposes scope, coverage, continuation and explicit refresh without treating unknown bytes as zero.

Implementation sequence:
1. Fix nested repository and artifact discovery, explicit roots and coverage (#1794, #1797, #1805). Test enclosing unborn Git repository, linked worktrees, unreadable/depth omissions and Swift output.
2. Add opt-in tool/cache providers and correct porcelain record limits (#1796, #1804). Test injected environments, deduplication and metadata-rich records.
3. Add resumable bounded measurement, efficient attribution, allocation floors and comparable growth (#1798–#1801). Test checkpoints, cancellation, changing scopes, hard links and partial readings.
4. Integrate scope/navigation and accurate size semantics (#1795, #1803); make discovery changes and explicit refresh update measurements (#1802). Test actual mounted pages and warm-cache/race behavior.
5. Review integration seams: native/TypeScript contracts, shared mobile surface, persisted old observations, per-path query identity, coverage wording and overlapping totals.
6. Run full Rust library, frontend, mobile and make lint gates. Keep issues updated, merge via green CI, verify main and all check attempts, tag v10.2.0 and verify published assets/signatures/version/notarization.

Development checkout and build outputs live under ~/code. Existing workspaces are retained. New findings receive issues linked to this epic where relevant; issues close only when fixes merge and acceptance is satisfied.

## Integration findings

- Added #1806: legacy artifact measurement could turn unreadable folders into zero bytes. Unknown readings now retain a qualified last measurement and cannot supply an idle age for cleanup.
- Coverage must travel with the accepted scan result; untagged completion events can arrive from an obsolete scope after refresh.
- Membership changes must preserve original measurement timestamps; otherwise copying rows into a new query key indefinitely renews stale data.
- Existing mobile clients require the original artifact command response shapes. New report commands carry richer coverage/nullability; old commands remain compatible and fail closed on incomplete measurements.
- Historical measurements are not necessarily current lower bounds. The UI labels retained/incomplete logical totals as last measured and does not promise reclaimed bytes.
- Large scans retain bounded discovery and measurement cursors in memory. Restarting the app retains saved observations but starts a fresh scan; changed filesystem entries restart affected measurement instead of mixing identities.
- Real-host acceptance added #1807: validating retained entries consumed the next batch's traversal quota, starving large scans near the batch limit. A regression reproduces the failure with a small quota; validation and traversal now receive separate bounded quotas under the same time limit.

## Read-only host acceptance

After fixing #1807, the existing large workspace completed in four batches: 572,082 entries across 36 locations, 200,142,893,056 allocated bytes (186.4 GiB), including 186,371,727,360 bytes of Cargo output (173.6 GiB). No incomplete coverage was reported. Runtime was 31.42 seconds; maximum resident set was 358,924,288 bytes. Paths are intentionally omitted. Final bounded-state verification follows before release.
- Added #1808: continuation previously retained unbounded per-entry metadata. Discovery and measurement now enforce conservative checked checkpoint-state budgets, retain known results at the limit, and report terminal partial coverage with narrower-root guidance. File deduplication reuses stamp indices instead of storing duplicate full paths.

Final capped host scan reproduced the same complete totals in four batches, with 264,634,368-byte maximum RSS and 49.52-second runtime while the full native suite also ran. The retained-path compaction reduced memory from the earlier 358,924,288-byte measurement.

Final local regression results: 3,539 native library tests passed (59 ignored); 5,249 frontend tests passed (1 skipped); 285 mobile workspace tests passed (1 ignored); the explicit real-host acceptance test passed. Native scanner/provider/accounting and frontend refresh/coverage integration were reviewed independently. Full lint and CI/release evidence follow in the PR and epic.

## Cross-platform CI follow-up (#1810)

Linux CI exposed order-dependent bare-repository ownership when nested discovery enters a linked checkout first. The deterministic linked-only and reversed-root fixture failed locally before the fix. Discovery now reads the bare marker from Git porcelain, excludes the owner container from worktree rows, and deduplicates bare owners through the same common-directory set. All 176 scanner tests pass after the correction.
