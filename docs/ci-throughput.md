# CI throughput and queue operations

CI retains the nine required status names. `lint` is an always-running aggregate
that requires both `lint-static` and `lint-native` to succeed. Failure,
cancellation, missing results and skips do not count as passing lint.

Linux runs static lint, frontend/Chromium contrast tests and dependency audits.
macOS still runs native Clippy/formatting, desktop tests with three race runs,
app/DMG and Intel checks, and native iOS validation. Linux and Windows platform
jobs, Android checks, and macOS native crash acceptance retain their coverage.
The shared setup action defaults to its original Node+Rust behavior. Node-only
jobs set `rust: 'false'`; native lint uses `node: 'false'`.

## Queue operation

Recommended repository ruleset values: build concurrency 1; status-check timeout
120 minutes. These are server-side settings, not values changed by this PR.
Keep all required checks and squash merging enabled. Increasing the timeout
prevents premature eviction; it does not make execution faster.

Use GitHub's actual Remove from queue action to withdraw a PR. Converting a PR
to draft or disabling auto-merge did not remove existing entries in the observed
incident. Confirm queue membership after any change. Do not replace this with
closing/reopening PRs, rebasing, empty commits or force pushes.

## Obsolete-run maintenance

`ci-maintenance.yml` runs trusted default-branch code on Linux after a CI run
completes, every ten minutes, or by manual dispatch. It never checks out a PR
head or downloads an untrusted artifact. Its cancellation permission is limited
to its job; normal CI retains read-only default permissions.

It cancels only an active run whose event is `merge_group`, workflow path is
`.github/workflows/ci.yml`, source is this repository and branch is an exact
`gh-readonly-queue/main/pr-...` ref. Immediately before cancelling, it rereads the
run and ref. Only a missing ref (ref-specific HTTP 404) or a different valid SHA
proves obsolescence. Failed or malformed API responses stop maintenance. An
existing matching ref is always preserved. Never use elapsed time alone to
cancel work. A ref GitHub leaves behind conservatively keeps the run alive.

Scheduled maintenance is excluded from the release product-check gate, like the
existing cache-budget job. No product CI check is newly ignored.

## Measure before changing capacity

Maintenance summaries show per-attempt job queue, execution and setup durations.
Queue time uses the first actual step start because GitHub may fill job
`started_at` while no runner is assigned. Dependency waits are included. Missing
or inconsistent timestamps are unknown, never zero. Stage details and existing
command diagnostics distinguish compilation from test work where recorded.

Compare the same change across several runs: required checks and test counts,
queue/execution p50 and p95, cache-hit rate, total runner minutes, and obsolete
runs cancelled. Separate warm and cold caches. Do not promise a completion time
until runner wait is bounded. Maintain main-only cache writes and the existing
global quota; old job-class budgets remain during cache expiry.

The inspected 2026-10-21 macOS logs identified GitHub Hosted Compute Agent,
MacCloud, westus. They did not identify the user's MacBook. Before switching to
self-hosted hardware, verify runner groups, architecture, Xcode and labels.
Use isolated disposable runners for public PR code, never the personal account
or signing keychain. Benchmark one and two heavy jobs before adding capacity;
use distinct work/build directories and track memory pressure and swap.
