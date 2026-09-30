# GitLab slice 8: created-cohort statistics checkpoint

2026-09-23. Implementation: Astra/high. Independent Astra/xhigh review pending.
This is an incremental slice 8 checkpoint, **not the full slice exit**.

## Implemented contract

GitLab.com has its own statistics page and, in Both mode, a separate provider
button. No count, mean, query key or persistent receipt is shared with GitHub.
The slice 6 integration must pass its persisted `selection` into
`ProviderStatsPage`; this standalone checkpoint defaults to GitHub. Integration
must hide the GitHub scope sidebar while the GitLab tab is selected.

Scope discovery reads membership projects with bounded pagination. Project
namespaces known to be groups are offered as group scopes. This is discovery
among membership projects, not an assertion that all token-visible groups and
projects have been enumerated. My authored MRs remains a distinct scope.

Statistics describe MRs **created in the selected 1–90 day UTC window**,
including today. Created counts, current opened/closed/merged counts, daily
created series, author counts, mean time from creation to merge, current
reviewer assignments, and MR history all describe this same cohort. A merged
count here does not mean all MRs merged during the window. Mean merge time is
withheld unless cohort pagination and every merged timestamp are complete.
Reviewer assignments are explicitly not review activity; actual review events
and response times are unavailable. Missing arrays and timestamps never become
zero. Partial counts and day values are lower bounds, including `at least 0`.

Each discovery/load has at most ten 100-row pages, a 45-second paging budget,
and ten seconds per request. A separate ten-second `/user` read resolves the
credential's stable numeric account ID. The host is always explicit and only
`gitlab.com` is enabled. Later-page failure preserves measured rows; first-page
failure rejects without inventing an empty result. Missing pagination headers,
contradictory totals, duplicate/malformed rows, and skipped page numbers cannot
produce complete coverage. Rate limit remaining/reset headers and stop causes
survive the wire and cache.

The new SQLite table is additive. JSON keys include provider, host, stable
viewer ID, scope and UTC date window. Five-minute receipts include coverage and
history. The UI requests fresh reads on mount/refresh and does not retain its
query cache after unmount; the backend supports short-lived cache reads for
other consumers. Cache errors do not erase successful network answers. This
is snapshot history, not an accumulated backfill ledger. Existing GitHub
history and cache tables retain their original meaning.

Both desktop command registrations and phone read allowlists/dispatch include
`gitlab_stats_tree` and `gitlab_stats_load`. No token is read by this adapter,
logged, persisted, or transported to the phone; `glab` owns authentication.

## Outstanding slice 8 work

- Merged-in-window series/counts across MRs created before the selected window.
- Review-event counts, reviewer participation, first-review/response latency,
  and review outcome measures. Assignments alone cannot establish any of these.
- Historical accumulation/backfill, per-slice ledger, incremental resume, and
  retained history outside the current bounded receipt. No background API spend.
- Richer scope hierarchy beyond groups represented by discovered membership
  projects (parent groups, external token-visible scopes, people scopes).
- Live large-cohort measurements, paging mutation behavior, populated reviewer
  events, rate-limit headers under actual throttling, and timeout fixture evidence.
- Self-managed host/version/permission validation.
- Assembled selector/sidebar navigation and paired-phone smoke validation.

## Evidence

Synthetic Rust tests exercise missing measures, host/path rejection, dedup,
timeout preservation, first-page failure, rate limits, unknown pagination, and
cache partition/coverage persistence. Component tests distinguish measured zero
from partial lower bounds and suppress missing/biased means. They do not claim
live GitLab API validation. Existing slice 0 supplies GitLab.com list/header
shape provenance; its two-draft fixture does not validate merged cohort metrics.

The [GitLab merge requests API](https://docs.gitlab.com/api/merge_requests/)
documents `created_after`, `created_before`, created scopes and project/group
list endpoints. This checkpoint uses those REST filters with explicit scope.

## Review corrections to the checkpoint

The Astra/xhigh review of `6fa932b` requested two P2 fixes. HTTP 429 responses
now carry their rate headers even when the error body is non-JSON. Later-page
429s replace the receipt's previous remaining/reset values; missing headers
clear those values to unknown. Retrieved rows and corrected coverage survive
cache persistence. A synthetic two-page regression covers both header cases.

Refresh now starts a new scope discovery before mounting statistics again,
including retrying partial project discovery. Scope selection is associated
with the discovered host/account; changing accounts resets the selection to
My authored MRs and removes the previous account's project options. A stats
response with a different viewer or source is rejected before the query cache
can assign it to the discovery account's key. Component regressions exercise
same-account partial discovery retry, account-switch refresh, and a switch
between discovery and the stats response followed by recovery.

Follow-up focused evidence: 8 Rust stats tests and 9 Vitest tests (6 GitLab
statistics component tests plus 3 surface guards) passed. Full release gates
and independent re-review remain separate requirements; this correction does
not close the broader slice 8 work listed above.

## Remaining Stats implementation, 2026-09-24

Implementation: Astra/high, based on integration `b949583`. The following
supersedes the implementation gaps above; independent review and full gates
remain required.

The original created cohort remains intact. A second bounded request loads
`state=merged` with `merged_after` and `merged_before`, independent of creation
time. It supplies its own count, daily series, author board, coverage, timestamp,
and retained MR records. The UI never adds these counts to created-cohort
counts. Invalid/missing merge timestamps reduce coverage and suppress means.
A failed merged read preserves the already measured created cohort.

Comment participation reads at most ten created-cohort MRs, one page of up to
100 notes per MR. Non-system comments by someone other than the MR author count
as participation; these can include bots. It reports comment and participating
MR counts and first-response time among MRs with a response. Pagination, skipped
MRs, malformed/duplicate notes, missing timestamps and unavailable endpoints
qualify counts and suppress latency. No successful note request means unknown,
not zero. First-page 429 evidence survives and stops further comment requests.
Approvals, changes requested and formal review outcomes remain explicitly
unavailable: comment activity cannot establish these measures.

Created, merged and comment reads share a 45-second paging budget. Requests
retain the ten-second timeout. The maximum is twenty MR-list pages plus ten
note pages; `/user` remains a separate account-discovery request. Authentication
and rate-limit errors use static messages. An account switch between scope
discovery and a stats/history response is rejected by the frontend key check.
The adapter does not freeze glab's external credential store during a load.

Group discovery now separately pages the token-visible groups endpoint,
including nested group paths, and retains its own partial/error evidence.
Membership projects and discovered groups are combined in the scope selector;
a validated username can be queried as a person scope. Project and group
paging share the discovery budget. Self-managed hosts remain disabled by the
integration's GitLab.com-only auth/capability gate.

History is now an explicit user request, `gitlab_stats_backfill`, available on
desktop and phone. Each click visits one closed UTC day. A new additive SQLite
ledger partitions receipts by provider, host, numeric viewer, scope and day.
Failed attempts have a timestamp and error with no fabricated report. Untouched
days take priority; afterward the oldest incomplete receipt is retried. Complete
created/merged day receipts are skipped on resume. A less successful retry
preserves prior measured cohorts with their own snapshot timestamps. Full MR
records persist, while the phone receives at most ninety day summaries rather
than retransmitting all retained MR rows. No automatic/background API spend is
enabled. Historical comment coverage is separate from count completeness.

Completed receipts are historical snapshots, not a claim that GitLab's search
index or MR state can never change. There is no automatic revalidation policy
or page cursor persisted inside a dense day. A day exceeding the ten-page bound
remains partial; the ledger can continue through other dates. Concurrent calls
may redundantly request the same incomplete day. Live large-cohort paging,
mutable-index behavior, populated comment samples, throttling, and self-managed
versions still need fixture evidence before the broad slice exit is claimed.

Focused evidence before final documentation/summarization changes: fourteen
Rust Stats tests passed (zero failed); thirteen Vitest tests passed across the
GitLab Stats component and desktop/phone surface guards. TypeScript, targeted
ESLint, knip, formatting and diff whitespace checks passed. An additional
zero-budget/429 comment regression and bounded history wire summaries were
added afterward and still need a Rust rerun. Heavy Rust builds were paused at
the driver's request because the shared Mac had 1.9 GiB free. Full `cargo test
--lib`, `yarn vitest run`, `make lint`, `make test-mobile`, CI, and independent
Astra/xhigh review remain gates; none is implied by this checkpoint.

API contracts checked against the official [merge requests API](https://docs.gitlab.com/api/merge_requests/)
and [discussions API](https://docs.gitlab.com/api/discussions/). Tests added here
use synthetic fixtures and do not claim live permission or tier validation.

## Review corrections, 2026-09-24

The fresh Astra/xhigh review of `5ebda65` found two P2 defects. A later-page
429 without rate headers now stops all following cohorts and comment reads by
its `RateLimited` stop reason; absent headers do not revoke that evidence.
Invalid-row qualification preserves the stop reason. A synthetic regression
checks both created and merged pagination, verifies the exact request count,
and includes duplicate rows so validation cannot accidentally re-enable work.

Daily history writes now take an SQLite IMMEDIATE transaction, reread the
current receipt, and merge measured cohorts before committing. The caller uses
the resulting persisted receipt. A slower failed request cannot erase a
completed day or return a locally stale missing-day result after a concurrent
success. A coordinated two-writer regression reproduces that ordering. Latest
attempt metadata and each cohort's own evidence/timestamp remain separate.

Stats tree, load and backfill now have typed wrappers in `src/api/tauri.ts`, used
by the query/mutation hooks, satisfying the desktop command reachability
contract that direct `transport.call` sites elsewhere do not satisfy.

After these corrections: thirteen focused Vitest tests, TypeScript, targeted
ESLint, knip, formatting and whitespace checks pass. Rust regression rerun and
fresh review of the changed diff remain required at this checkpoint.
