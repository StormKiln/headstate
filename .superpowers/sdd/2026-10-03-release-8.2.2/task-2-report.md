# Task 2 — advisory display retention and demand (#1654)

Status: DONE_WITH_CONCERNS (verification limits below; no known failing gate).
Review base: `2b4bb3c8321e94c0a283eb95baf91f98ae30ff86`.
Root's intervening `b281526` updates the plan only. Task2 does not change inventory observations, stack merge authorization, native admission limits, foreground content/review paths, source versions, or release metadata.

## Result and integration API

- `useReadyStacks.of(pr)` remains freshness-only. `PrDetailView` still sends only this value into `pr.stack` and therefore action planning. New `displayOf(pr)` returns `{value, freshness: 'fresh'|'retained', observedAt}` separately.
- `useReadyPushers.of(pr)` remains freshness-only for `partitionReady`, filter context, and approval consequences. New `displayOf(pr)` returns the same qualified display envelope containing `ReadyPusher`. A retained viewer pusher never auto-hides a row or says its approval cannot count.
- Actual `ReadyStrip` consumers now render retained stack and pusher chips. They visibly say “last known” and include the original observation time in accessible explanations. `ReadyStackChip` accepts optional `retained` and `observedAt` props. Task4 may compact/reposition these presentations; it must preserve these semantic distinctions and fresh-only partitioning.
- `useReadyStacks(prs, priority?, enabled?, consumer?)` now has an explicit fourth argument: `'strip'` (default) or `'detail'`. `PrDetailView` passes `'detail'`. This identifies ancestry advisory demand, not foreground PR content or review writes.
- Evidence remains owned by query state, not a retained-value WeakMap. Account changes, viewer removal/reset, and same-viewer re-pair/reset clear it. Generation and abort checks prevent queued/dispatched old-session responses from publishing. Full identity includes owner/generation, provider/host/repo/PR, head/base, and pusher head repository/ref.
- Native cache slots now keep latest attempt and last successful observation separately. Failed/declined loads do not erase success or reset its observation age. Fresh-only native `load`/`peek` never return the retained channel. The existing native 512-slot cap and singleflight remain.

### Additive wire fields

Existing commands and argument shapes are unchanged. Optional response fields:

- RowPusher: `pusher_valid_for_ms`, `rules_valid_for_ms`, `last_known_pusher: {value, age_ms}`, `last_known_rules: {value, age_ms}`.
- RowStack: `last_known_stack: {value, age_ms}` in addition to existing `valid_for_ms`/head/base.

Native remaining lifetime is read under the receipt lock. Frontend deadlines are anchored at actual dispatch, not IPC arrival. Policy and pusher expiration are separate (native 600s and60s respectively). Legacy missing lifetime fields cannot establish current authority, including when an older still-unexpired receipt exists. Retained native channels carry elapsed age, never fresh permission. Generated remote wire schema was regenerated with `make wire-contract`; desktop/mobile compatibility gates passed.

## Demand architecture and root-approved adjustments

The original independent windows failed a real native mixed scenario: selected nonnative detail produced no usable result in105 cycles while pusher 8 + strip-stack 8 ran concurrently. Six-priority/two-fair selection alone could not fix that starvation. Root approved a small shared advisory dispatch queue:

1. One shared30s clock per QueryClient aligns mounted consumers, including late-mounted detail.
2. Selected ancestry goes ahead of waiting strip work. Already-dispatched work is neither preempted nor replayed.
3. Single-row pusher/strip-stack IPC alternates, with the first strip class also alternating across completed drains. An eight-row native batch cannot spend all actual attempts before another lane asks.
4. Window order is preserved through dispatch. Visible work is preferred; a fair offscreen row leads on alternating windows so an unreadable visible prefix cannot monopolize admitted opportunities.
5. Known offscreen identities stop demanding reads. Visible/selected mutable facts revalidate after their original TTL. Missing/changed identities are bounded by the live window. Unrelated canonical-set churn does not eagerly choose another batch.
6. Hidden documents unsubscribe from the clock and remove observers; queued aborted work is discarded. Existing dispatched native work may complete within its original deadline.

The initial2h query GC was replaced, with root approval, by capacity-only retention (`gcTime: Infinity`, hard512/type eviction). A constrained fair lane plus failures can take more than 2h to cover275 rows; elapsed-time GC would recreate the defect. Eviction favors inactive oldest entries, falling back to oldest active identities if necessary to preserve the hard cap. Original native freshness remains finite. Tests cover530 identity replacements and520 simultaneously observed stack queries, including retention beyond 3h while the session remains active. Viewer/session removal still clears retained evidence.

Memory bound: at most512 pusher +512 stack query identities per QueryClient; native retains512 slots per rules/pusher/stack cache, each with latest and last-success values. These are cardinality bounds, not measured heap-byte claims. Queued work derives from live finite windows; the normal single ReadyStrip + detail has at most8+8+1 demand identities before query coalescing. Multiple consumers coalesce matching query identities; no process-wide evidence cache is introduced.

## RED evidence (real production hooks/native boundaries)

All commands run in `/private/tmp/headstate-8-release-gate`, unless specified. Rust uses `CARGO_TARGET_DIR=/private/tmp/headstate-pr1596-review/src-tauri/target`.

| Command | Observed RED result |
|---|---|
| `yarn vitest run src/api/advisoryEvidence.test.tsx` (initial5 tests) | 5 failed: display disappears after failed refresh/oldGC; native-aged pusher still hides; newly visible demand does not get a turn;20 unrelated population toggles create20 batches. `/private/tmp/task2-red-frontend.log` |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib strip_receipt_preserves_native_remaining` | Real wiremock response lacks `pusher_valid_for_ms`; expected remaining-lifetime assertion fails. First sandboxed attempt could not bind localhost; authorized escalation then produced the behavioral RED. An earlier cache API draft also hit a compile-time missing-method error; that is not counted as behavioral evidence. |
| `yarn vitest run src/components/ReadyStrip.test.tsx -t 'keeps retained stack'` | Real ReadyStrip has no retained stack label. Later age assertion also failed before observation time was added. `/private/tmp/task2-red-ui.log`, `task2-red-age.log` |
| `yarn vitest run src/api/advisoryEvidence.test.tsx` after initial retention implementation | Known offscreen identity requested twice instead of once. `/private/tmp/task2-red-offscreen.log` |
| `cargo test --manifest-path src-tauri/Cargo.toml --lib github::gates::tests -- --nocapture` | 26 passed,1 failed: simultaneous pusher 8/stack 8/detail ancestry yielded105 consecutive selected failures. Actual attempts stayed<=8/cycle. `/private/tmp/task2-native-gates.log` |
| `yarn vitest run src/api/advisoryEvidence.test.tsx -t 'late selected'` | Next shared window started pusher instead of late selected detail before shared-clock change. `/private/tmp/task2-red-clock.log` |
| `yarn vitest run src/api/advisoryEvidence.test.tsx -t 'fair offscreen lane ahead'` | First strip-stack remained inside unreadable visible prefix before preserving/interleaving window order. `/private/tmp/task2-red-fairlane.log` |
| `yarn vitest run src/api/advisoryEvidence.test.tsx -t 'alternates the first strip'` | Even-sized complete drains always restarted pusher first. `/private/tmp/task2-red-alternate.log` |
| `yarn vitest run src/api/advisoryEvidence.test.tsx -t 'legacy reply with'` | Missing-lifetime reply borrowed an older receipt's lifetime and still auto-hid the row. `/private/tmp/task2-red-legacy.log` |

Regression coverage also includes contradictory same-head stack/pusher/policy success replacing retained display, expired-on-arrival authority, hidden/resume behavior, queued cancellation behind a slow in-flight request, same-viewer reset, alice/bob-style login round-trip with delayed old completion, duplicate observers, head/base/ref/repository invalidation, finite capacity, and selected-detail fresh action separation.

One original frontend test exposed timing sensitivity: `PrDetailView.refresh.test.tsx > lets retained conversations expand while their mutation controls stay paused` passed at BASE in `/private/tmp/headstate-task2-baseline`, but the new query observer timing let its assertion run before the error render. It now awaits the real error alert before checking Reopen is disabled. No action behavior was relaxed. Initial full frontend run had4814 passed/1 failed with that exact failure; it is fixed and included in the final green suite. Initial lint surfaced stale generated wire output and two new lint errors; generation, awaiting test `act`, and moving clock reads into the effect resolved them.

## GREEN gates and measured counts

- `yarn vitest run`: **324 files,4824 tests passed**, zero failures. `/private/tmp/task2-full-frontend-final.log`. The suite emits its existing jsdom “Not implemented: navigation to another Document” notice.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib`: **3155 passed,52 ignored**, zero failures. `/private/tmp/task2-full-native-final.log`. Includes native cache, gates, stack/merge/action, admission, actual transport concurrency/reserve, and remote tests.
- `cargo test --manifest-path src-tauri/Cargo.toml --lib sustained_mixed_large_owner -- --nocapture`: final native stress replay counts recorded below. `/private/tmp/task2-native-mixed-final.log`.
- `CARGO_TARGET_DIR=/private/tmp/headstate-pr1596-review/src-mobile/target make test-mobile`: **224 tests passed** across workspace suites (181+16+13+14), zero failures; doc/empty suites also pass. `/private/tmp/task2-mobile.log`.
- `make lint-ui`: passed generated wire freshness/tests, TypeScript, ESLint, knip, focus CSS and shell-lock checks. Existing34 FastRefresh warnings remain, no errors. `/private/tmp/task2-lint-ui-final.log`.
- `cargo fmt --manifest-path src-tauri/Cargo.toml` and `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`: passed. `/private/tmp/task2-clippy-final.log`.
- `git diff --check`: passed.

### Sustained actual request budget

The unchanged native cap is8 actual advisory HTTP attempts per30s with at most2shared background/advisory requests in flight and existing foreground reserve. Therefore the sustained ceiling is16 attempts/minute or960/hour; retries and native fanout count against it. Warm pusher+ordinary nonnative leaf stack typically need1+2=3HTTP attempts, plus one policy read for a cold/expired repository/base. Eight row demands are not eight HTTP attempts.

Native replay uses275 synthetic PRs across44 repos, nonnative stack membership requiring two real HTTP requests, and one selected detail. It advances Tokio time30s after each cycle and replays315 cycles (157.5 minutes of synthetic clock; nine nominal35-window rotations). It intentionally continues asking strip identities to stress admission even after success; real frontend known-offscreen suppression therefore generates less demand. Selected commands are also repeated each cycle, but native60s receipts are reused: only158 selected downward reads occur across 315successful selected results, not315 fresh selected fetches.

The pre-final replay measured2520 actual HTTP attempts over 315 cycles, all275 distinct pushers and all275 distinct stack identities observed,315/315 usable selected results, maximum selected unsuccessful gap 0. The final replay additionally pre-spends8 actual attempts and proves a late selected request is declined without exceeding that period, then succeeds first after the next30s advance. Final total includes those8prelude attempts; the315-cycle phase remains capped8/cycle. Final measured counts: **2528 total HTTP attempts (8 exhausted-period prelude + 2520 during 315 cycles),158 selected downward reads,275 distinct pushers,275 distinct stacks,315/315 usable selected results, longest selected unsuccessful gap 0**.

The old uncoordinated105-cycle version produced0/105 usable selected results; the initial coordinated105-cycle version produced840 attempts,219 distinct pushers,166 distinct stacks and105/105 usable selected results. Extending to315 cycles demonstrates eventual coverage of all275 for this healthy synthetic workload. It is not a convergence promise for arbitrary provider failures or rate-limit cooldowns.

Concurrency evidence is distinct: production JS serializes advisory commands; a single row's native work is sequential. Native admission remains2 for independently submitted advisory/background work. Full-suite `github::read_transport::tests::scoped_clients_share_limits_and_preserve_foreground_stats_slots` exercises real delayed HTTP, proves only two background/advisory requests occupy transport while foreground requests retain their slots. The replay's HTTP counts come from wiremock received requests, not IPC counts.

### Selected-detail ancestry advisory opportunity bound

First demand is enqueued immediately and moves ahead of waiting strip work after the current command. Previously refused/stale demand is reconsidered at the next shared30s tick. Hook tests demonstrate late mounting, shared-clock promotion, coalescing, and waiting behind a live slow request without replaying it. The final native replay demonstrates exhausted-period refusal followed by an opportunity after 30s.

A conservative **inferred**, not real-user-measured, bound for one selected ancestry consumer with a healthy IPC/provider and no cooldown is80s: allow two30s ticks because the native admission epoch can lag the frontend clock, plus up to10s for existing native work and10s for the selected command. Provider errors, cooldowns, transport disconnection or multiple selected consumers can prevent a successful fresh observation; retained chips remain qualified and destructive actions stay unavailable. This is NOT PR-opening or approval latency: foreground content loads and review writes remain outside this queue.

## Self-review, concerns and limits

- Reviewed all Task2 diff paths, query ownership/eviction, dispatched-vs-arrival anchoring, current-vs-retained consumers, same-owner reset, queued cancellation, native last-success semantics and generated wire changes. Found and fixed the first-class drain fairness and legacy lifetime-borrow defects with RED/GREEN regressions.
- No live provider traffic or enterprise account data used. The native replay and frontend hooks are separate test layers, not an end-to-end React-to-running-Tauri benchmark. No claim of measured live-user latency, CPU, memory bytes or throughput.
- Capacity-only retained evidence may be old; chips explicitly qualify it and disclose observation time. Fresh action/filter TTLs do not extend. Hard-cap pressure can evict useful active identities, which safely becomes unknown rather than permitting stale actions.
- Failures/declines preserve last success; successful contradictory observations replace it. Native success classification uses the established cache contract that failure TTL is5s and successful TTLs are longer (60s/600s).
- Task4 owns final compact status layout/sorting; Task6 should retain mixed-demand and action-boundary regressions. Root owns independent review, remaining release-wide gates and GitHub/release actions. No version bump or publication performed here.
