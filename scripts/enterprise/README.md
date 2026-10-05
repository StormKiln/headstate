# Synthetic enterprise acceptance

This opt-in driver boots a real zero-window Wry application, production scheduler,
read admission, SQLite stores, command services and authenticated paired mTLS/SSE.
The browser mounts production `ReadyStrip`, `PrList`, `PrDetailView` (including
`ReviewThreads`, `ReviewBox` and `PrActions`) and `StatsPage`. Desktop commands use
an authenticated test bridge; paired commands cross the real listener. Neither
adapter returns canned product answers. Filesystem/Claude controls alone are
explicitly disabled through `localTools={false}`; ordinary app defaults stay true.

It does not exercise packaged Tauri invoke/WKWebView IPC, the installed phone
Rust proxy or mobile suspension. It never runs normal application setup,
keychain, tray, notifications, updater or autostart. Synthetic profiles have a
marker; an existing unmarked database is refused. Never point it at a real
profile. The opt-in Cargo feature and required-feature binary are absent from
ordinary app selection (`default-run = headstate`).

## Run

Use the repository Node/Yarn/Rust toolchains and installed Playwright engines.
No real account or GitHub credential is needed. Commands require local browser,
WindowServer and loopback access on macOS. All output directories must be new.

```sh
make enterprise-ui enterprise-driver
make test-enterprise-contracts
ENTERPRISE_DRIVER="$PWD/src-tauri/target/debug/enterprise-driver" \
  make enterprise-run OUT=/tmp/enterprise-new ENGINE=chromium MODE=fault
```

`ENTERPRISE_DRIVER` overrides the local binary path; set it when using a shared
Cargo target. Modes: `gate` (mounted actions and continuity), `fault` (rate,
partial/offline recovery, outer-write abort, pairing/owner retirement and reply
ordering), `retirement`, `contention` (real SQLite writer), `crash` (SIGKILL inside
outer transaction), `offline` (retained restart), `load` (approval during held
production historical backfill), `baseline`, `soak`. For restart only, set
`PROFILE` to a retained synthetic profile; it is copied, not overwritten.

```sh
make enterprise-baseline OUT=/tmp/enterprise-baseline-new
make enterprise-compare OUT=/tmp/enterprise-regression-new
ENTERPRISE_BASELINE=scripts/enterprise/baseline.json \
  make enterprise-run OUT=/tmp/enterprise-comparison-new MODE=baseline
make enterprise-run OUT=/tmp/enterprise-soak-new ENGINE=webkit MODE=soak
```

The baseline batch captures five cold profiles and five warm copies per engine,
with both roles in every run. Its checked-in measured data includes actual
commit, dirty state, source patch hash, seed, engine versions and host/build
metadata. Comparison only accepts equivalent startup populations; do not compare
whole fault/soak document totals with startup totals. Timing/count envelopes are
chosen from measured maxima and remain regression thresholds, not enterprise
SLAs. WebKit unsupported long-task observations are null. Five samples support
median/range, not a robust p95. Action pending/confirmed samples are separate.

The soak uses real 60-second production cadence for at least 30 minutes. It
extends up to 45 minutes for external close/merge, all demanded closed-day history
and Ready pusher/rules/stack observations, then fails explicitly if unfinished. Extend the
run policy with evidence if more time is required; do not claim convergence from
inventory alone. It also checks complete cache reads and a 125-second unchanged
active-detail window. No accelerated clock or repeated wake/refetch loop runs in
the measured soak. Manual detail invalidation used for thread-movement tests is
labelled manual; cross-role approval uses automatic production source publication
and detail revalidation.

## Evidence

The provider models 50 repositories and 50 members including the viewer, 50
viewer-authored open PRs and 150 review-requested open PRs with no cross-list
overlap (no impossible self-review). Both roles share the same lists. Another
250 merged PRs occupy yesterday’s paginated closed day (the concrete UTC date is recorded in each manifest). Scope/filter/identity assertions
are independent of UI counts. Separate REST/GraphQL capacities are 5000 with
120-second windows and a known synthetic cost of one per received document.
These costs do not predict GitHub costs. `aliases` means root aliases only.

The bounded native recorder (65536 pending events), provider ledger and browser
arrays (100000 entries each) invalidate a run on loss. Read submissions, received
headers, body terminals and Stats Spend are separate; pre-header failures may
have zero Spend. Reconciliation checks scope terminals, four read/two background
ceilings (writes separate), and provider receipts against actual submissions.
Slot release is not equivalent to producer cancellation. Intentional crash runs
retain explicit unfinished scopes instead of claiming clean shutdown.

Retain metadata, raw native/provider/browser ledgers, SQLite/DB-WAL profiles,
traces, screenshots, exit/shutdown and failure results. The private config
contains only generated synthetic bridge credentials and is written mode 0600;
do not publish it. Browser traces never carry the bridge secret (the Node proxy
adds it server-side). Audit artifacts before public export. Failed attempts are
retained, not silently replaced by later passes.

CI runs deterministic fixture/budget contracts and the isolated real transport
observer regression alongside production Rust fault tests. Full native browser
fault/soak runs remain explicit supported-host gates; a green CI unit suite alone
is not integrated acceptance. Baseline values are same-host profiling-build
observations and must not be treated as macOS runner latency SLAs.
