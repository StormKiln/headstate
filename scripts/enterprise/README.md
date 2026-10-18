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

Checkpoint publication emits path-free `checkpoint-staging` records: raw byte
buffer capacities, dirty digest vector/string capacities, link vector backing
(`capacity * size_of<PrLink>`) plus owned link strings, metadata string capacity,
and the 32-byte digest conversion scratch. `checkpointEncodedBytes` covers the
checkpoint's raw byte/digest delta and metadata, not link SQL parameters. Header
encoding duration is separate from transaction wait/hold. The 24 MiB measured
publication envelope is an observation threshold, never an evidence-eviction
rule or a total-memory guarantee. Fixed stack bookkeeping, corpus/offset scheduling collections, parser-owned trees,
read-phase clones/buffers, SQLite copies and RSS are outside this payload scope.
Oversized valid historical metadata remains lossless and may exceed current
parser bounds. `claude_link_bytes`, `claude_link_hash` and
`claude_link_continuation` are explicitly classified in transaction observations
and the profile inspector. Run `make measure-enterprise-checkpoints` for the
retained synthetic ordinary-build probes; see the release checkpoint measurement
document for before/after evidence, headroom and initial-WAL tradeoffs.

Application bytes are counted at returned reads for metadata, usage, search index
and links. They include rereads, not physical I/O. The population inventory additionally emits path-free `discovery` records with
returned directory entries, cooperative work units, completed directory passes,
authority, elapsed time and selected owned payload bytes. Payload coverage is
explicit: directory name/certificate and returned-scan session/source strings; diagnostics, tree nodes, SQL,
transient allocations and RSS are excluded. Ordinary builds have no recorder
calls. Legacy foreground enumeration, stat/fingerprint calls, SQLite reads,
network traffic and unscoped writes remain unknown. One-second and boundary native samples report inclusive process RSS,
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

The mixed adapter explicitly brackets `idle`, `unchanged_refresh`,
`active_ingestion` and `churn_restart` with authenticated controls named
`resource-<phase>-start` and `resource-<phase>-end`, and collects the existing browser
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
capability remains unknown. The wrapper uses schema2, contract `mixed`, and exactly
ordered `initial`/`resumed` native process segments. Each segment retains schema1
raw native/browser evidence under `mixed_initial` (four phases) or `mixed_resumed`
(`churn_restart` only). Startup schema1 is unchanged. Different native PID and
handshake nonce are required. Native clocks, sequence namespaces and cumulative
counters remain local: active durations/read bytes/mutations sum, peaks take the
maximum, and memory drift remains per segment. No offset or continuous memory
trend is invented across restart. The batch requires these additional witnesses:

- `resourceIdentity`: generated `manifestSha256`, exact `toolchain`, and `phasePlan`
  mapping the four phase names to summed fixed requested durations, plus exact
  `segmentPhasePlan` durations. `calibrationEligible` must be true.
- `mixedOutcome`: independent product/provider/corpus outcome facts, equal across
  observer A/B arms. This must come from actual mixed-workload assertions.
- Existing normalized workload/environment fields, `warm`, the exact native binary
  SHA-256, and independent `parentObservation` fields. Source revision may change as
  provenance; host, browser, build class, workload, phase plan and capability must match. Each local phase duration must be within the requested duration and 2.5 seconds above it; larger scheduling overruns reject comparability. The combined restart phase allows the sum of those two local tolerances.

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
not a valid startup/resource baseline. Set `ENTERPRISE_MIXED_PHASE_PLAN` to the approved fixed-plan JSON before a batch;
exploratory runs deliberately cannot calibrate. No calibration envelope is claimed
by helper or exploratory tests.

`make measure-enterprise-discovery` runs only the two exact synthetic discovery
probes, retains their generated fixtures, and asserts zero transcript body reads.
It never selects a home corpus. The inventory's immediate transactions include
`claude_corpus_epoch`, `claude_corpus_directory` and `claude_corpus_inventory` in
the fixed-name row-callback allowlist. See
`docs/superpowers/specs/2026-10-18-discovery-measurements.md` for calibration,
production-cadence convergence and the portable restart/observation limits.

## Seeded sustained churn and checkpoint restart

`ENTERPRISE_REVIEWING=150` or `250` and optional `ENTERPRISE_SEED` select a
validated descriptor (`churn-v3`, 50 authored PRs, 50 repositories/members).
Run `make enterprise-run OUT=/tmp/churn-new ENGINE=chromium MODE=churn`.
This completes revisions 1–7 of the saved plan, including real desktop/paired
navigation and successful current-head approvals. A successful segment is **not**
a completed plan. Resume its remaining revisions with the same scale/seed:
`make enterprise-run OUT=/tmp/churn-resumed ENGINE=chromium MODE=churn-restart PROFILE=/tmp/churn-new/profile`.
Repeat both phases for WebKit and each scale for the final acceptance matrix.

The plan issues work at least 75 seconds apart. Foreground actions retain a
180-second ceiling; background changes use the versioned scale/kind-specific
healthy envelope below, with a separate 180-second meaningful-progress watchdog.
The production 60-second scheduler remains unchanged.
There are no wake/refetch loops. Both mounted clients must converge against an
abstract oracle initialized from the seed, never provider-returned rows or the
product eligibility selector. External arrivals, heads, requests, drafts, queue
membership and terminal transitions use a separate provider adapter. Approvals
advance the oracle only on one successful delivered actual mutation receipt.
The held PR 53 draft, disclosure, focus and backward selection must survive
background refresh; deliberate navigation precedes that retention interval.

`churn-plan.json`, `churn.json`, and `churn-checkpoint.json` retain nominal/actual
times, event and request counts, accepted production row facts/source receipts,
remaining obligations, and independent plan/oracle/provider/profile hashes.
Restart rejects failed, unclean, incompatible or tampered sources, replays the
oracle without provider data, checks byte-identical copied profile evidence,
and reads authenticated cached authority for both roles before polling starts.
All failures and profiles remain in their unique output directories. This is a
finite churn qualification; the existing fixed 145-row fairness soak is separate.

Final sustained admission calls `assertSustainedChurn(result.churn)`: it requires
all plan revisions, no remaining obligations, and at least 30 measured active
minutes across validated segments, excluding shutdown/restart downtime. A final
restart waits autonomously for this duration while continuing observation.
`ENTERPRISE_CHURN_INTEGRATION_ONLY=1` skips that final duration wait for adapter
smoke work and is recorded in metadata/result; it can never qualify as sustained
acceptance even when every event converged. Task 9 must reject this flag.

Mixed workload consumers should reuse `createChurnPlan` / `initialOracle` /
`advanceOracle` and `assertChurnFacts` for independent `mixedOutcome` evidence,
`applyChurnEvent` only for external fixture changes, and real successful
mutation receipts for actions. `loadChurnRestart` restores provider state only
after validating and independently replaying the saved oracle;
`captureChurnCache` verifies provider-free native cache authority. Do not
recreate expected identities from provider rows or browser-derived eligibility.
The sustained admission helper is exported from `churn-plan.mjs` for final
matrix/resource/release consumers.

Churn v3 execution serializes every immutable plan obligation: issue spacing is
at least 75 seconds and the prior obligation must converge before the next issue.
Nominal offsets remain unchanged; `nominalDriftMs` records actual drift. The
versioned healthy-fixture policy is a conservative finite envelope, not expected
latency or a production SLA. Foreground navigation/approval keeps 180 seconds.
At 150/250 requested rows, background positive changes allow 510/690 seconds,
terminal disappearance 540/720 seconds, and request removal 600/780 seconds.
Actual timings and progress remain in `churn.json`; these limits never add sleep.

The derivation uses `S = ceil(ceil((reviewing + 1) / 25) / 2)`: 25 rows/page
(`github/client.rs`), two pages/step (`github/scan.rs:389`), and the one scheduled
arrival. Two traversals cover finishing the in-flight pass and a fresh pass.
Each continuation cycle allows two independently measured <=15-second list
steps plus the production 15-second sleep (`poll.rs:1166–1189`), hence 45 seconds.
A further 120 seconds covers completed-pass cooldown (`queue_scan.rs:317`) and
ordinary 60-second cadence. Positive publication allows 30 seconds; terminal
identity confirmation allows another 30 seconds. Request removal adds the
production 60-second separation between two negatives (`github/scan.rs:742`).
Thus the kind-specific limits are `2*S*45s + 120s + 30s/60s/120s`.

A separate 180-second watchdog requires affected-list cursor/frontier change,
accepted affected-identity semantic fact/qualification change, or at most two complete exact-identity
negative confirmation proofs. Repeated requests or changed timestamps alone
cannot reset it. Correct facts and exact obligations remain mandatory at the
endpoint. Finite segment bounds sum each obligation's deadline plus spacing.
Restart checks exact version/hash and source timings; incompatible old sources
require a fresh run, never a tolerance or migration.

Scheduled quiet intervals sample existing UI witnesses at most one second apart,
with no provider refresh. Both roles are checked independently; any observed
post-convergence regression or retained interaction loss is fatal. Native step
work includes scan-slot creation through scope drop and associated accepted
publication. Unfinished/unaccepted slots are included. The native file reader
consumes only new complete lines with bounded carry/backlog. Recorder producers
allocate sequence IDs before sending, so line arrival order can differ from
sequence order; records are reordered with bounded buffering. Timestamps are
checked against same-slot starts, not globally across concurrent producers.
Acceptance and final slot drop can arrive in either order; the later native
boundary defines duration. Final proof rejects missing/duplicate sequences,
unpaired/unfinished lifetimes and loss. Task 8/9 consumers should preserve this
recorder concurrency contract and raw trace evidence.

Affected-fact progress uses an explicit projection: identity/list/head/authored,
request/draft/queue/terminal/approval, and observation state, unknown/retained/detail
field sets, ready-at qualification and confirmed-review head/review/unresolved/read
qualification. It excludes every timestamp and receipt metadata. Field sets are
canonicalized; previously seen semantic values cannot renew progress by alternating.


### Actual mixed corpus and restart workflow

`make enterprise-run OUT=<new retained directory> ENGINE=chromium MODE=mixed`
runs an exploratory cold source and actual process restart. Optional
`ENTERPRISE_MIXED_WARM=1` first runs an independent unmeasured real process to
populate the initial corpus and unapproved PR cache. Every warm sample prepares
its own baseline; it never copies a previously mutated cold profile. Preparation
requests, wall time and parent observations are retained separately. A fixed phase
plan has `schema:1` and `segments.initial` (idle, unchanged_refresh,
active_ingestion, churn_restart) and `segments.resumed` (churn_restart), durations
in milliseconds. Until native timing supports and the controller approves those
values, omit the plan and retain `calibrationEligible:false`.

The profile remains at one marked generated path for both native processes.
After clean shutdown, the initial segment saves an immutable whole-profile copy
and hashes; restart checks those bytes, source result, independent oracle,
manifest and exact native/UI/harness provenance before opening the database.
Persisted PR approval/request and corpus authority are checked with zero provider
IO. Durable pause survives restart; only the explicit resume control wakes the
ordinary population worker. Complete raw native recorder validation precedes
resource selection; concurrent recorder arrival ordering follows the Task7
bounded reorder and per-slot lifecycle contract above.

Before fixture bodies exist, an authenticated one-time control invokes the real
empty rooted import to establish production link baseline authority. A repeat,
nonempty root or existing corpus authority is rejected. The marked root contains
128 normal transcripts plus large, malformed and unreadable cases. Only existing
stored production coverage/search consumers are mounted; no HOME discovery UI is
enabled. Both clients open and approve current-head PRs during population and
read actual FTS results. Exact token/link/index/source facts come from the
independent fixture manifest. The fixed synthetic link-checkpoint trigger must
actually fire; prior link/checkpoint/byte/digest authority remains unchanged while
metadata, index and usage advance. Actual permission-read failure is required.
Owned shutdown removes the trigger and restores permissions, cancels and joins
blocking work before recording a clean exit. Fault/control receipts are harness
effects, separately retained from product observations.


Frozen mixed resource limits include a stable key for every allowlisted native
callback table and insert/update/delete count, plus transaction counts. The table
list is checked against the native recorder allowlist. After validation of lossless
scoped raw transaction evidence, an absent callback key means zero callbacks in
that scope; unscoped writes, WITHOUT ROWID and other declared callback holes stay
unknown. Unknown or unclassified table callbacks reject calibration/validation.
Count headroom is twice the observed maximum plus one occurrence, with an observed
zero locked to zero. Size/time headroom remains separately unit-specific. Per-segment
positive RSS median growth is budgeted independently; it is not a continuous trend
across process restart.


Mixed provenance also records actual Node version, module ABI and executable hash;
these participate in source/restart and toolchain equality. The owned shutdown
wait retains its finite join timeout but clears the losing timer after a quick
exit. Baseline `corpus-pass` native scopes wrap the actual blocking population
call, mark the real returned indexed count, and exclude cadence sleep. These
three records are identical with the optional resource observer disabled. Cold
integration requires an actual PR detail/review native command interval, identified
by the UI reply callId, to overlap a completed positive-work pass on the same native
clock. Warm/resumed overlap is reported only when observed; their stored-cache,
append, recovery and restart obligations still apply. The cold scenario opens its
PR before the legitimate worker start, then approves during pending population;
there are no forced ticks or injected delays.


Mounted consumer checkpoints validate actual `claude_coverage` and
`claude_index_coverage` replies plus the rendered production coverage text for
both roles: initial baseline, permission degradation, durable-pause cancellation,
and recovery. Cold degradation has130 stored/indexed rows of131; a prepared warm
corpus retains131 accepted rows while population remains130completed/1remaining.
Discovery EACCES is proved separately: the index unreadable array stays empty and
the consumer must state incomplete population. Pause is rendered as the actual
`Scan cancelled` qualification, with native durable pause separately checked.
Recovery is131/131 and still shows the one-transcript8MiB truncation caveat. The
ordinary coverage polls remain unchanged; matching FTS hits cannot substitute for
these coverage assertions.

Every process must supply measured positive parent RSS and command latency before
mixed aggregation; missing/null values reject instead of becoming zero or hiding
behind another process. Every native sample's cumulative lane counter is checked
in native timestamp order, including unscoped intervals. Missing/malformed values
and any intra-process decrease reject even if the endpoint recovered above the
start. Resets between distinct processes remain legitimate. Feature-only native
sample snapshot and emission are serialized together so periodic/control-boundary
races cannot emit an older counter snapshot with a later timestamp. The disabled
observer returns before this lock; unrelated recorder events retain their existing
concurrent ordering contract.
