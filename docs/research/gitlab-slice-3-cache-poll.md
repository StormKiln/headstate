# GitLab slice 3: source-scoped cache and poll channel

This slice adds the cache, status, and manual-refresh contracts. Production
network fetching remains GitHub.com-only. GitLab.com fetching is slice 4;
self-managed hosts remain unvalidated and disabled.

Migration 30 rebuilds `snapshot` with `(provider, host, id)` as its key. The
existing authored and reviewing rows retain their JSON and original fetch
timestamps and become `github` / `github.com`. Payloads without `source`
continue to deserialize as GitHub.com. Old snapshots have `unknown` coverage,
because they never recorded whether their results were complete. New writes
reject rows from a different source. GitHub list evidence retains optional totals,
refused fields, failed pages, and missing page counts before legacy wrappers apply
numeric defaults; none of those unknown/partial cases becomes complete. Reads suppress a mismatched or corrupt
payload instead of relabelling it.

The Rust helper `get_source_snapshot(source, list)` returns an envelope identifying the source
and authored/reviewing list. Its data is `missing`, `unreadable`, or `available`.
Available data includes the stored fetch timestamp, stale age, rows, and
`complete`, `partial { total }`, or `unknown` coverage. An empty available list
is a measured empty queue; neither missing nor unreadable data is a zero.
Legacy cache commands retain their existing GitHub-only shapes.

The Rust helper `get_source_poll_status(source, list)` and the `source-poll-status` event carry
independent phases, consecutive failure counts, errors, last network receipt
times, and coverage. Reading a cache never advances network freshness. Phases
are `not_requested`, `fetching`, `ready`, `partial`, `unknown`, `retrying`,
`failed`, and `not_asked`. A first transient failure is retrying; a second or a
nontransient failure is failed. Failure keeps previous receipt/coverage. A
success clears only its own source/list error. The latest started attempt owns the pending/error phase. The newest successful
attempt owns the usable data; a later failed request cannot discard that data. The source gate stays held through every
Fetching and terminal status emission, including refusal and error paths. Status is process-local; on reconnect, query
the status command and cache command rather than assuming an event arrived.
The source event is forwarded to the phone. These source helpers are not yet
registered IPC commands: registration, surface classification, dispatch, and
TypeScript wrappers belong with slice 6's actual frontend consumers.

The Rust helper `refresh_source(source, list)` targets one queue. Both existing
foreground GitHub refresh commands use it and persist coverage. Authored refresh
retains its existing 30-second outer limit. Reviewing retains its existing
page loader with bounded individual HTTP requests, without a new outer timeout
that would discard already fetched pages. A cache write failure does not discard fetched rows.
Unsupported providers/hosts and absent GitHub authentication are explicitly
`not_asked`, before a network request is issued. GitLab authentication remains
independent and does not enable fetching by itself.

The existing GitHub background loop publishes this status for both queues,
retains partial coverage, and persists a successful review queue even when the
authored queue fails. Its existing 45-second shared GitHub tick budget remains.
Notification and mergeability-recheck comparisons now use full row identity;
notifications include the source host. Partial notification baselines suppress
arrival claims on the next complete load rather than announcing rows omitted
by the partial result as new arrivals.

## Integration seams

- **Slice 4:** add a GitLab.com branch in `refresh_source` and a separate GitLab
  background task/waker. Give that task its own request/tick budget; never share
  GitHub's remaining tick budget. Use `source_poll::begin`/`finish` and
  `save_source_snapshot` with the explicit source/list and coverage. Acquire
  `source_poll::publication` before writing and hold it through terminal status.
  The bounded
  page loader must return already retrieved pages on its own deadline, with
  partial coverage; wrapping an accumulating future in a timeout would discard
  those pages. Do not infer GitLab readiness from GitHub notification predicates.
- **Slice 6:** register the source cache/status/refresh helpers as IPC commands,
  classify and dispatch them in both surfaces, add typed frontend wrappers with
  actual consumers, consume the status event, and add
  source-scoped query keys, and render provider/list freshness and failure
  independently. The existing frontend and legacy events still show GitHub only.
  Add GitLab/Both source selection before starting the GitLab poll. Combine rows
  only at the display boundary, preserving each queue's coverage and error.
- Publication uses a per-source/list async guard. Successful responses compare
  against the newest successful generation, so a usable response may be saved
  while a later request is pending or failed. It cannot replace a newer success.
  The receipt and timestamp advance while the later attempt's pending/error phase
  remains intact. Delayed rechecks retain their originating generation.
  Foreground callers can immediately adopt a newer retained success; there is no
  network join or unbounded successor wait. Disk publication remains serialized.
- Every successful reviewing publication emits `reviewing-updated`, paired with
  `reviewing-short` (a number when measured, null when unknown). Both events reach desktop and
  phone. `useReviewing` consumes the row event and preserves it if an older
  foreground query subsequently resolves or rejects. An unknown total emits null to replace previous numeric advice with a generic
  completeness qualification; source coverage retains the unknown/partial state. Legacy authored error/state events stay inside the
  publication permit, including restoration of a later failure after older usable
  rows are published.
- There is no provider-scoped store-error UI yet; the legacy disk failure banner
  remains global. Source-specific data errors use the new channel. Stats/history
  partitioning and paired-phone offline storage of GitLab rows remain later work.

## Validation

Added Rust regression tests cover legacy migration preserving both payloads and
timestamps, independent provider/host/list freshness and coverage, overlapping
identities, rejecting a wrong-source cache write, corrupt versus absent versus
empty data, independent error/retry/recovery state, pending versus not-requested,
not-asked versus failed, superseded status attempts, and notification/recheck
identity collisions. Existing cache migration and GitHub poll tests remain.

Local checks are limited to Rust formatting, whitespace validation, and direct
SQLite execution of the migration due to the user's thermal constraint. Full
Rust/mobile suites and lint are delegated to the Linux validation host by the
parent task, and independent Astra/xhigh review is still required. No live
GitLab queue fetch or self-managed support is claimed.

The first Linux test run compiled and reported 2,437 passed, 4 failed, 48 ignored.
The failures identified three incomplete historical schema fixtures and three
scaffold-only commands registered without desktop consumers. The follow-up fixes
restore realistic snapshot tables to those fixtures and defer the registrations.
Three additional tests exercise publication ordering/isolation and missing-count,
refusal, failed-page coverage. Their execution awaits the next Linux run.

The next review found that joining a successor could exceed the foreground
budget, and an unpublished fallback could regress an A/B/C completion sequence.
The final design removes the join and publishes successes in generation order
independently of the latest attempt's failure. Rust tests now cover a stalled or
aborted successor and B success after C failure before older A completes, with
snapshot readback. Three frontend cases cover later background reviewing rows,
shortfall delivery, an older foreground success/failure, and listener cleanup.
The Linux run before these changes passed 2,447 Rust tests (48 ignored); its
Clippy clone-to-slice findings are corrected in the notification test. Latest
changes still require Linux verification and fresh review.

The frontend consumes source status for authored errors, so an older successful
manual request cannot dismiss a newer failed attempt. Explicit user dismissal is
still available. The reviewing query no longer converts a latest foreground
failure into success just because an older background event arrived during it.
Nullable shortfall events replace stale numeric advice on both lists; unknown
completeness is displayed without a count. Transport coverage includes both new
events. Focused tests cover these transitions, source isolation, and both list
warnings; Linux validation and fresh review remain the final gate.

The reviewing hook now retains its source's latest error independently of
TanStack's data cache. An older background row event can therefore update useful
rows without clearing a newer foreground failure, in either completion order.
A real later source success clears the failure; authored/provider events cannot.
The review view shows the failure alongside retained rows instead of replacing
them. Two hook ordering cases and one application rendering case cover this fix.

Once the reviewing hook observes source status, it treats both its error and its
recovery as authoritative, including when an older command rejection arrives
after a ready event over the phone's separate connection. It keeps observing rows
and status while its queries are disabled on other views, so returning within
the query freshness window cannot revive an error whose recovery was missed.
Two focused hook regressions cover these sequences; Linux verification remains
with the parent task.

### Correlated command replies and phone event delivery

`refresh_now` and `get_reviewing` accept an optional `requestId`. Without it,
the existing array/error contract is unchanged. With it, provider outcomes are
returned as `{ request_id, update }`; IPC rejections are transport failures.
The phone forwards the optional argument through its existing command routes.
A newer frontend also accepts legacy arrays from an older paired desktop.

The source update includes a process session, monotonic status revision,
independent receipt revision, latest rows, and the completed request ID on
terminal events. These fields are transient and never enter the SQLite cache.
Replies take an atomic snapshot under the publication gate; rechecks advance
receipt revision even when the attempt generation has not changed. The shared
frontend reconciler orders replies/events and keeps a transport failure until
its own outcome or a later requested refresh arrives. Unrelated background
successes can update rows without concealing a failed phone request. A missed
recovery event is repaired by the next correlated reply. Existing legacy row
and poll-error events remain for older clients; the current frontend uses the
versioned update once observed.

Slice 4 should reuse this publication path for GitLab receipts. Slice 6 can
extend the source selection in the shared observer without inventing another
reply/event reconciliation rule. Focused coverage includes late successful and
failed authored replies, reviewing transport failure before/after unrelated
background recovery, missed event recovery, process changes, legacy arrays,
and status/receipt ordering. Full validation remains on the Linux host.

Confirmed mutation patches now update the reconciler's authoritative rows as
well as TanStack data; enqueue/draft/ready/approval remain visible during a
pending or failed readback. A correlated failure with no receipt preserves
absence instead of installing an empty live list, retaining the SQLite rows and
their stale marker. The SSE hub retains the latest versioned source frame per
source/list even with no subscriber and replays it after the legacy SQLite
snapshot on reconnect, recovering missed rows/status without another poll.
Focused hook, reconciler, and SSE wire regressions cover these three cases.

Completeness advisories now follow the accepted versioned receipt's coverage,
including SSE reconnect replay. Authored lists show a measured total; reviewing
lists show the measured missing count. Unknown or partial coverage without a
positive measured gap uses the generic warning. A newer complete receipt clears
it; an older frame or a separate legacy advisory cannot change the qualifier on
current versioned rows. Legacy advisory events remain the fallback before a
versioned receipt is observed. Reconnect tests cover counted partial, uncounted
partial, unknown, and later complete recovery for both lists.
