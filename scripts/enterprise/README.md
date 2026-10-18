# Synthetic enterprise acceptance

This opt-in driver boots a real zero-window Wry application, production scheduler,
read admission, SQLite stores, command services and authenticated paired mTLS/SSE.
The browser mounts production `ReadyStrip`, `PrList`, `PrDetailView` (including
`ReviewThreads`, `ReviewBox` and `PrActions`). Desktop commands use
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
Cargo target. Modes: `authored` (actual PrList and authored detail in both roles), `gate` (mounted actions and continuity), `fault` (rate,
partial/offline recovery, outer-write abort, pairing retirement and reply
ordering), `retirement` (pairing revocation/repair), `crash` (SIGKILL inside
outer transaction), `offline` (retained restart), `load` (approval during held
production reviewing-search transport), `baseline`, `soak`. For restart only, set
`PROFILE` to a retained synthetic profile; it is copied, not overwritten.

```sh
make enterprise-baseline OUT=/tmp/enterprise-baseline-new
make enterprise-compare OUT=/tmp/enterprise-regression-new
ENTERPRISE_BASELINE=scripts/enterprise/baseline.json \
  make enterprise-run OUT=/tmp/enterprise-comparison-new MODE=baseline
make enterprise-admission-probe OUT=/tmp/enterprise-rejected \
  BASELINE=/tmp/intentionally-incompatible-schema-2-baseline.json
make enterprise-run OUT=/tmp/enterprise-soak-new ENGINE=webkit MODE=soak
```

New baselines use schema 2 and carry one normalized workload descriptor plus a
per-engine environment descriptor. Admission requires exact seed, scenario,
role, population, cadence, schedule, quota, host, browser and build compatibility
before measured work starts. Source revision is retained as before/after
provenance and may differ. The historical schema-1 baseline remains immutable
and is rejected explicitly rather than compared under incomplete semantics. The
admission probe takes an intentionally incompatible schema-2 baseline and proves
the real runner rejects it with zero provider receipts while retaining clean
native shutdown evidence.

The baseline batch captures five cold profiles and five warm copies per engine,
with both roles in every run. Its checked-in measured data includes actual
commit, dirty state, source patch hash, seed, engine versions and host/build
metadata. Comparison only accepts equivalent startup populations; do not compare
whole fault/soak document totals with startup totals. Timing/count envelopes are
chosen from measured maxima and remain regression thresholds, not enterprise
SLAs. WebKit unsupported long-task observations are null. Five samples support
median/range, not a robust p95. Action pending/confirmed samples are separate.

The soak uses real 60-second production cadence for at least 30 minutes. It
extends up to 45 minutes for external close/merge, Ready pusher/rules/stack observations, then fails explicitly if unfinished. Extend the
run policy with evidence if more time is required; do not claim convergence from
inventory alone. It also checks a 125-second unchanged
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
headers and body terminals are separate; pre-header failures have no response receipt. Reconciliation checks scope terminals, four read/two background
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

PR Stats and its convergence, cache, demand-lease, account-owner retirement and
Stats-triggered writer-contention scenarios are retired. Existing synthetic
history rows, profiles, baseline data and screenshots remain retained evidence.
Every current run records `statsRetirement` with zero received historical/Stats
documents and zero retired bridge commands; a nonzero count fails acceptance.
Classification runs before injected provider faults, so failed requests count too.
Warm replay accepts retained legacy close-day metadata without mounting or calling
Stats; new runs retain exact external close/merge timestamps.

### Weighted natural quota (`MODE=quota`)

`make enterprise-run OUT=/private/tmp/headstate-quota ENGINE=chromium MODE=quota`
uses the ordinary native driver, mounted desktop/paired clients and production
60-second polling cadence. Build with `make enterprise-driver enterprise-ui`
when needed. Use a new output directory for every attempt; failed artifacts are
retained. WebKit uses the same mode with `ENGINE=webkit`.

The opt-in `weighted-natural-v1` profile in `quota.mjs` is synthetic accounting,
not a forecast of GitHub prices. Its complete parameters and canonical SHA-256
identity are recorded in `result.quotaModel` and `result.workload.quotaProfile`.
REST and GraphQL have independent capacities (800 and 12000) and 150-second
windows. Parsed GraphQL field/page/detail/advisory/mutation weights and REST
endpoint weights determine cost before response materialization or mutation.
A shared rolling cost burst window can naturally emit secondary 429/cooldown;
focused provider/model tests exercise this separately from primary recovery.

Each received weighted document has `quotaShape` and an immutable request-time
`quota` receipt: bucket, window identity, capacity, before, requested cost,
charged cost, after, reset, and natural/secondary refusal. Accepted requests are
charged even if a later fixture/transport fault fails. Denied requests charge
zero and cannot mutate provider rows. A primary over-cost refusal advertises
zero **admissible** remaining capacity in the protocol header while its receipt
preserves the actual unspent remainder. Receipt replay checks conservation and
window/reset/burst behavior; scenario validation independently recomputes cost
from the recorded parsed request shape. Document counts remain separate from
`quotaAccounting.charged` totals.

The scenario crosses the real REST `<=500` reserve and proves that a mounted
GraphQL approval can still succeed. This is independent-bucket foreground
usability: the production reserve applies to **all REST reads**, and the harness
does not introduce a REST foreground-read bypass. Bounded alternating desktop
and paired foreground detail acquisitions then naturally exhaust GraphQL.
Read-only observers verify blocked background refresh, retained accepted
identities, native cooldown, bounded traffic, a later real reset receipt, and a
new accepted source publication consumed by both mounted clients. Previously
accepted 50/150 inventories are retained through incomplete refresh; this mode
does not claim an initially partial list subset. Existing partial-page fault
and convergence modes remain separate.

No quota mode step toggles `fault.mode`, changes admission thresholds, repeatedly
wakes the scheduler, or advances an artificial clock. The default equal-document
and convergence fixtures retain their existing policy. `startProvider` accepts
a normalized workload plus optional quota profile, rejects a mismatched profile
identity, and rejects unsupported population descriptors rather than advertising
250 PRs while serving 150. Generalized fixture populations belong to the churn
extension; the current fixture remains deterministic and seed 9001 is recorded
as workload provenance.

### Resource observations (10.9 foundation)

`make test-enterprise-resources` checks bounded raw-phase contracts and scoped native
accounting; `make test-enterprise-corpus-resources` checks the actual instrumented
production corpus call sites. `make lint-enterprise-resources` checks the opt-in
feature, which ordinary builds do not compile. `make enterprise-run OUT=<new path>
ENGINE=chromium MODE=baseline` retains the startup resource smoke; WebKit uses the
same command with `ENGINE=webkit` and an explicitly unknown renderer heap.

Native resource records share `native.ndjson`'s bounded recorder, sequence, monotonic
clock and loss bit. Hooks are installed only for the exact observed Connection and
removed on scope exit. Nested acquisition fails without replacing an outer hook;
other connections cannot contribute. Only successful COMMIT publishes callbacks.
Rollback, failed BEGIN/COMMIT and rollback failure are separate outcomes. Counts
are SQLite callbacks, including named FTS shadow rows; WITHOUT ROWID, truncate
DELETE and implicit REPLACE deletions are coverage holes, not logical row counts.
Queue transactions retain deferred BEGIN: wait measures BEGIN admission, while hold
includes possible later writer-upgrade contention. Corpus scopes retain immediate
BEGIN. Records carry the actual transaction mode, and contracts require that mode.

Application bytes are counted at returned reads for metadata, usage, search index
and links. They include rereads, not physical I/O. Directory enumeration,
stat/fingerprint calls, SQLite reads, network traffic and unscoped writes remain
unknown. One-second and boundary native samples report inclusive process RSS,
DB bytes and retained WAL length; absent WAL stays absent. The marker-gated profile
inspector separately reads counts and PRAGMAs. Neither path sums WAL lengths as
writes. Chromium CDP reports telemetry-inclusive renderer JS heap; WebKit,
application-only and browser-process memory remain unknown. Result artifacts
separately report recorder capacity/output bytes, renderer telemetry/commit entry
counts, trace sizes, and parent Node RSS. No memory subtraction is performed.

Startup comparisons now require raw resource boundaries and raw renderer
observations: queue milestones, absolute detail milestones, actual commit arrays
and long-task arrays. Detached `samples` are rejected even when under budget.
`result.resources.evidence` holds raw resource records, `summary` must reconcile
those records, and `startup` holds the browser observations used to derive startup
metrics. `resources.json`, the original native recorder, renderer artifacts and
parent sampling file remain retained. Recorder loss, required unknown probes,
missing boundaries, incompatible coverage, duplicate sequences and native sampler
gaps over 2.5 intervals reject the result.

The mixed adapter in Task8 must explicitly bracket `idle`, `unchanged_refresh`,
`active_ingestion` and `churn_restart` with authenticated controls named
`resource-<phase>-start` and `resource-<phase>-end`, and collect the existing browser
resource observer using `startPhase(phase, control)` and `endPhase(phase, control)`.
These methods issue the native boundary controls and record explicit browser
boundary samples for each role. `stop()` returns raw samples plus Node monotonic
request/response brackets; `bindBrowserClock(clock, nativeRecords)` associates each
bracket with the exact native boundary sequence/time. The validator requires
contiguous unique browser sequence numbers, finite monotonic timestamps, one
start/end sample per role/phase, at most 2.5 intervals between samples, and complete
boundary coverage. Native and Node phase duration must reconcile inside the
request/response brackets (2 ms clock rounding allowance); no guessed clock offset
is treated as exact. WebKit follows the same sampling/coverage protocol while heap
capability remains unknown. It must produce `resources.evidence.contract = "mixed"`, the
same schema/coverage/sampler fields and a reconciled summary. Startup is not a
substitute for mixed load. The batch foundation requires these additional witnesses:

- `resourceIdentity`: generated `manifestSha256`, exact `toolchain`, and `phasePlan`
  mapping the four phase names to fixed requested durations in milliseconds.
- `mixedOutcome`: independent product/provider/corpus outcome facts, equal across
  observer A/B arms. This must come from actual mixed-workload assertions.
- Existing normalized workload/environment fields, `warm`, the exact native binary
  SHA-256, and independent `parentObservation` fields. Source revision may change as
  provenance; host, browser, build class, workload, phase plan and capability must match. Actual mixed phase duration must be within the requested duration and 2.5 seconds above it; larger scheduling overruns reject comparability.

`make enterprise-resources OUT=<new batch> RESOURCE_STAGE=calibrate` reserves five
cold and five warm Chromium fixed150 mixed observations on macOS arm64. It derives
separate cold/warm limits and freezes them to `limits.json`, with explicit headroom.
`RESOURCE_STAGE=validate LIMITS=<frozen limits.json>` makes five independent cold and
five independent warm runs and never rewrites those limits. `RESOURCE_STAGE=overhead`
makes five A/B pairs, alternating order. A uses `ENTERPRISE_RESOURCE_DISABLED=1` with
the same compiled binary; it skips resource hooks, counters, sampler, timing and CDP
measurement. The independent parent samples native RSS, Node RSS and DB/WAL in both
arms and reports wall time, artifact size and existing native command durations.
Tracing and ordinary product telemetry remain identical. Disabled observation is
not a valid startup/resource baseline. These commands deliberately reject until the
real mixed adapter supplies its witnesses; no calibration envelope has been claimed
by the foundation smoke tests.
