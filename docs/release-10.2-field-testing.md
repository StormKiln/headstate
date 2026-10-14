# HeadState 10.2 disk inventory acceptance

The release connects System Health, Artifacts and Worktrees through explicit disk
scope and qualified measurements. A scan does not delete files.

## Check discovery and accounting

1. Add a parent folder containing nested repositories, including a parent that is
   itself a Git checkout. Refresh Worktrees and confirm the nested repositories
   appear once, including linked worktrees and bare owners.
2. Open the disk inventory from System Health. Confirm the selected roots match
   the intended scope. Global developer caches and toolchains are opt-in; select
   them only when that additional scope is wanted.
3. Add an explicit custom build-output root. Check that recognized Cargo and
   Swift output appears with its measured category and ownership, or as an
   external location without an invented owner.
4. Continue a large partial scan. Known results must remain visible and advance.
   A checkpoint memory limit must retain qualified results and suggest narrower
   roots rather than claim the scan completed.
5. Create or remove disposable build output, refresh, and compare observations.
   Added, removed and incomparable locations must be distinguished. Unknown
   measurements must not become zero or make cleanup eligibility less strict.
6. Check the size labels: logical file bytes, allocated disk bytes and unknown
   reclaimable space are different quantities. Do not add overlapping Artifacts
   and Worktrees totals together.

Continuation state lives in the running desktop process. Restarting retains saved
observations but begins a new scan. A changed directory between batches can
restart discovery or measurement to avoid reusing stale offsets or allocation.

## Report unexpected results

Include the app version, operating system, scan completion/coverage messages,
number of locations, and whether roots or files changed between observations.
If sharing a log or screenshot publicly, remove private paths and repository
names. Keep original logs locally for diagnosis.

The pre-release read-only host acceptance covered 572,082 entries in four bounded
batches, finding 186.4 GiB allocated across 36 locations. This is one measured
workspace, not a guarantee of scan speed or a historical growth baseline.
