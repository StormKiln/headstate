# Measurement foundation API (schema 1)

The foundation includes Task 6A desktop To Review and Task 6B Stats/StopFailure
producers described below, plus Task 6C transcript and independent phone-local capture.
The analyzer and field runbook remain the subsequent 6D slice.
A successful export is not a performance or convergence pass.

## Construction and ownership

The shared `measurement` module has no Tauri, provider, SQLite or networking dependency.
`Recorder::new(Config { directory, epoch: [u8;16], role, platform, build })` starts one
writer. The shell supplies a secure random process epoch and an app-owned directory.
Initialization/entropy failure leaves diagnostics unavailable without failing startup.
Desktop `measurement_desktop` owns a `OnceLock<Recorder>` and binds the persisted
`ui.diagnostic_logging` preference at startup and after successful preference saves.
The companion compiles the same core with its independent local preference, adapter
and measurement UI described under Task 6C. Desktop commands are `Local` in both surface tables.

`enabled()` is an atomic fast path. `set_enabled(false)` immediately closes admission,
clears private keys/live handles, and requests a best-effort flush without waiting for
IO. Accepted records retain their original capture and drain in order. Re-enable
increments capture; handles never alias a prior epoch/capture. Reserved lifecycle
metadata records opened/closed captures even when no data records exist.

Native producer methods:

- `intern(Key::Owner(private_incarnation))`, `intern(Key::Session(private_incarnation))`;
  `intern(Key::StatsScope { owner, scope, from_day, to_day })` requires that the matching
  Owner key already be live. Include the real source/owner incarnation in native keys,
  not just a mutable login string. Private keys remain in memory only.
- `next_operation(parent: Option<&OpaqueId>)` creates a monotonic handle without
  consuming a lifetime key entry. Use an actual owner/session parent when known.
  `retire(&id)` invalidates the handle and descendant scope/operation/receipt handles.
  Complete operations must be retired by their real producer; no timer invents completion.
- `receipt_reference(&owner)` issues a bounded live reference for an actual accepted
  publication. Only issue it at that transition, attach it optionally to those rows,
  and retire the old reference when the rows are replaced. Task 6 owns the wire seam.
  Status-only updates must retain the row reference. Never synthesize one from a timestamp.
- `record(Event) -> bool` accepts only closed native events. False includes disabled,
  duplicate, invalid/stale handle, rate budget or queue refusal; these are not provider
  outcomes. The bool must never influence product read/write success.
- `aggregate(AggregateDelta { domain, metric, work, count })` coalesces counters.
  Values are totals, not event timestamps or averaged latency percentiles.
- `measured_count(u64) -> Option<u32>` performs checked narrowing. Overflow becomes
  unavailable and is counted while capture is enabled; zero remains a measured zero.
- `status() -> JournalStatus` reads published state without writer IO.
- `export_to(PathBuf).await` is for **trusted local shell adapters only**. One export
  at a time; the desktop shell owns its native save dialog, including cancellation.

Types are in `measurement/model.rs`; no generic text or JSON-map producer API exists.
QueueReceipt, Operation, StatsProgress, StopFailureMatch and Transcript are separate
native variants. Stats counts and elapsed are optional, and `covered_days` is not
complete days. StopFailure uses optional `observed_boundary` (capture-local sampled
ordinal), `FailureObservation` provenance, and explicit unpaired/ambiguous/censored
outcomes. It never invents a true turn identity or an unbiased latency population.

`ClientMeasurement` is the only client-input union: MountedReview, StatsView,
TranscriptView. Optional native references are checked for epoch/capture/liveness
**and kind**. A client cannot intern private keys, mint ownership, or use an owner
handle where a receipt/scope reference is required. Foreign phone references remain
unlinked; phone-local producers must omit them rather than forging local ownership.
Missing readings remain null/absent. MountedReview separates inventory, Ready eligible,
visible, membership-retained, retained-readiness, readiness-unknown, actual Last-known
and optional-advisory counts, plus actual footer location/state. Task 6A wires its production
predicates and bounded count traversal as described below.

## Bounds and loss

Production caps: 4096 private keys/1 MiB key bytes; 1024 active parent correlations;
1024 data records; 8 control requests; 1024 encoded bytes including newline per record;
32 client records/16 KiB per batch. Producers use bounded `try_send`, no disk IO.
The exact-duplicate cache keeps the most recent 128 event encodings per capture.
Event quotas use monotonic minute windows: 120 transitions/client observations,
independent 60 StopFailure observations, and 12 routine summaries. Queue bursts
cannot consume the StopFailure allowance. Overflow/refusal/loss is explicit.

128 aggregate keys retain cumulative counts and their original start when the routine
budget defers emission. A summary is attempted on a subsequent aggregate update after
60 seconds; no new product polling timer is introduced. Still-deferred totals are
included as bounded header metadata at export, explicitly not timestamped exact events.
Re-enabling another capture clears old pending totals with an explicit dropped count.
A bounded durable marker records whether deferred totals remain unpersisted. If the
process restarts, `deferred_aggregate_gaps` reports that omission and marks coverage
incomplete; it does not invent the number of lost events. Same-process disabled exports
still contain their actual deferred totals. In-flight summaries retain their aggregate
slot; admission and total subtraction are atomic with export snapshots.
Dedup/coalescing is disclosed separately from dropped/invalid data.

Writer flushes every 5 seconds or 128 records and on control barriers. This is not a
crash-loss guarantee under scheduler/IO stalls. Status exposes only durable records.
The export barrier has a 5-second bound; failure returns Timeout and the queued writer
must not publish its destination. Streaming after an accepted durable barrier does not
block product producers. Export and rotation run serially on that one writer.

Eight 16 MiB segments retain at most 128 MiB; one replacement temporarily permits
144 MiB. One export temporary is bounded by 129 MiB: combined payload working space
273 MiB, plus manifest below 64 KiB and filesystem allocation overhead. This is an
engineering cap, not a guaranteed number of days. Actual retained interval, rotation
and loss are visible. The reader bounds each line, rejects unknown versions/fields or
invalid references and omits malformed/truncated records with explicit accounting.
Restart reconciliation compares surviving files with prior durable segment metadata.
Missing/shortened durable data increments `durable_gap_segments`, known missing
`durable_gap_records` and `durable_gap_bytes`, and makes coverage incomplete. Extra
flushed records beyond the manifest remain usable; recorded rotation is not counted
again as an unexplained gap. Files use create-new staging, symlink refusal and 0600 permissions where supported.
An unclosed capture found after restart is explicitly qualified as an unknown tail
(`unclean_capture`), not an invented number of lost records. Writer failure latches unavailable; there is no retry spin or operation failure coupling.

## Commands, UI and file shape

`measurement_status`, `measurement_export`, `measurement_client_events` are desktop
local-only commands. Typed wrappers live in `src/api/tauri.ts`; the shared input/output
interfaces live in `src/types/measurement.ts`. Local commands are deliberately excluded
from the generated remote schema. Existing remote clients cannot export or write this
journal. Old unsupported commands remain errors, never an empty successful capture.

The actual desktop Settings General panel mounts `MeasurementExport`, refreshes status
on opening/preference change/explicit action, and uses the native picker. It can save
retained captures while logging is off. No periodic status timer and no automatic send.
The existing detailed timing log and its repository/PR privacy disclosure are unchanged.

Export JSONL:

1. `kind: header`, schema/build/platform/role, `cutoff_epoch` + `cutoff_seq`, actual
   first/last durable wall clocks, record count, retained epoch/capture counts, caps,
   immutable loss snapshot, current-epoch lifecycle and deferred aggregate metadata.
2. Validated schema-1 envelopes with epoch/capture/seq, monotonic and wall clocks,
   role and closed `event`.
3. `kind: trailer`, schema, matching cutoff/count, `complete: true`, and incomplete flag.

A complete file can still have incomplete observation coverage. The analyzer must refuse
exact lineage across loss, malformed/truncated records, unsupported schemas or mismatched
header/trailer/count. No cross-device monotonic subtraction; epoch changes are explicit
restart discontinuities. No journal bytes or destination path pass through JavaScript.

## Task 6A production observations

Native queue observations are explicitly scoped to GitHub; GitLab native queue state is
not measured. `QueueReceipt.receipt` is optional and distinct from `operation`; the core
validates its receipt kind, current capture, liveness and matching owner. SourcePolls
creates a reference only at an accepted row publication, retains it across status-only
updates, and retires it when rows or ownership change. The optional wire field is
`measurement_receipt`. Cached seed, replay without a live reference, and local row patches
remain unlinked. The frontend carries the reference with the accepted source rows, never
with an independently cached row array solely because its status revision advanced.

`Operation.operation_class` is optional `detail` or `action`.
`Operation.affected_fields` is optional: unmeasured counts are omitted, while existing
numeric values (including a genuinely measured zero) remain readable and unchanged. Actual awaited command
scopes emit start/completion and retire their handle, including cancellation and early
errors. Completed success means the command returned successfully. Action acknowledgment
is success only for an actual confirmed effect; successful but unconfirmed outcomes are
unknown. Published marks an accepted fact transaction. Awaited list publications inherit
the task-local operation; spawned work does not, and later background scans are unlinked.
No rejected attempt identity is inferred from the current receipt recorded at rejection.

`Domain::ReadTransport` contains shared account-client read transport aggregates, which
may include supporting or Stats reads. They are not a To Review-only population.
Admitted/refused and logical canceled/completed reads use existing decisions; completed
is not a claim of provider success. Advisory and live demand whose priority can change
are classified unknown. Queue aggregates separately observe existing cache reuse,
coalescing, not-issued and normal continuation/hidden/no-work decisions. No new demand,
request, database scan or timer is introduced. Legacy five-domain loss arrays remain
readable; the new sixth entry is shared read transport.

The opt-in desktop Ready panel records selected/all inventory, eligible and visible
lengths plus one pure traversal of at most 4096 selected rows for membership retention,
readiness retention/unknown and its actual Last known predicates. Above the cap those
row qualifiers are absent. Optional advisory count remains absent. The displayed desktop
footer branch is bound to the same source frame before an observation is sent. This is a
React commit observation, not a paint or user-perceived latency measurement. GitLab-only,
main-list rows and the phone ConnectionBanner are not measured by this slice; a hidden
desktop footer is explicitly hidden. Phone capture is independently default-off; Task 6C adds its local wiring.
Recording failure, stale references and budget loss cannot change product success.

## Task 6B Stats and sampled StopFailure observations

Stats observations reuse the immutable client's recorder and existing owner, query and
window. Registration is recorded after its existing transaction returns; `registered`
and `registration_failed` are separate from returned-board success. Foreground and
background `committed` observations follow successful history/coverage transaction
commit; `upsert` totals count actual returned written rows, including repeated corrections,
not unique insertions. A useful partial transaction can be committed while the tick fails.
`commit_failed` never claims written rows. Existing tick outcomes use bounded Stats
aggregates; existing progress reports supply counts without another database read.
Covered days are a union, not a partition. Partial/unknown-day counts remain absent;
checked u32/u16 narrowing counts overflow. Durations cover the observed request section
(after foreground owner capture, or the actual cache-only read), not provider latency.

`measurementScope` is optional metadata on existing StatsBoard/StatsBoardReadback replies.
It is never recovered from serialized board caches and never requires another RPC.
The native client tracker holds at most 32 question slots; changing a question's exact
window retires its old scope. A changed captured owner generation retires that client's
old owner and descendants; dropping the client also retires them. Older in-flight owner
generations cannot restore the tracker's owner. Concurrent different windows for the same
question can retire an earlier window's diagnostic reference: the earlier answer remains
usable but correlation becomes unavailable/counts stale, never a reason to refetch.
Core capture/handle limits still apply and refusal leaves the optional reference absent.
No diagnostic code queries current ownership or acquires demand. Cache-only reads remain
zeroHTTP/zero-registration/zero-demand, including while capture is disabled.

StatsView adds optional closed `observation: readback | mounted`; legacy omission means
unspecified. Readback events occur after the existing controller acceptance/retention
fences; rejected numbers and references are absent. The mounted StatsPage observes the
actual accepted board at React effect/commit, with its author-row count (not PR population,
viewport rows or paint timing). Retained data keeps its own scope reference, and older
responses without metadata remain unlinked. Mounted elapsed time is absent. It subscribes
to the app's existing preference cache without fetching preferences, Stats or progress.
Task 6C also enables phone-local preference/recording and omits foreign host scope
references; they are never interpreted as phone-local authority.

StopFailure ingestion is observed only after the real consume transaction commits, using
the existing INSERT OR IGNORE row count for duplicate replay. The bounded staging list
holds at most512 descriptors referencing already parsed records. Missing/invalid source
timestamps stay absent/clock-anomalous even though legacy storage supplies a timestamp.
Failed commits/exclusions contribute `StopFailure/Declined`, not committed hook events.
The real digest observes its already gathered list/registry/newest-failure map before the
unchanged 15-second classifier and before display filtering. No extra registry/DB read,
provider call, notification, or timer is introduced.

The instance-owned join retains at most128 sessions, four hook observations and four idle
boundaries per session for ten monotonic minutes. Native-only session/process strings are
capped at1024 bytes each; they are never event labels. An observed boundary ordinal is
capture-local, not an upstream turn ID. First idle, registry gaps and process changes are
censored. Repeated unchanged samples are coalesced; they do not create a new boundary when
the join buffer expires. A current registry/newest-history comparison after expiry remains
retrospective/censored with no invented ordinal. Candidate `matched` means one retained
candidate after a sampled busy→idle observation, not proven turn membership. Multiple
retained candidates are ambiguous; old hooks remain visible through signed delta and
out_of_window/censored provenance. Hook age and idle observation lag are separate from
the signed hook-time minus idle-time delta. Future/invalid source times never wrap or
become a measured unsigned age. Both ingestion-first and sample-first orders are observed.

StopFailure `eviction` aggregates count retained hook/boundary entries removed from
correlation memory by expiry, per-session capacity, process change or reset. They are not
lost journal events, numbers of true turns, or counts of sessions. Reasons share this
closed category; it cannot distinguish those causes after export. `declined` counts input
observations excluded before retention/commit (including capacity refusals), not provider
failures. Disable closes admission first, clears private join state, and retains the reset
count in the existing bounded deferred metadata of that capture. Re-enable discards old
capture aggregates using the foundation's counted policy; it cannot join old handles.
These records describe sampled/censored observations, not an unbiased latency population.
The independent60/minute StopFailure quota and all existing classifier/action authority
remain unchanged. Shutdown/crash tails retain the foundation's unclean-capture limits.

### Capture lifetime after preference changes (#1740)

The desktop's existing `set_ui_prefs` call can negotiate `measurementCapture: true`.
After a successful saved preference change it returns only `{epoch, capture}` for an
active local capture, otherwise null. Calls without negotiation still return null;
the remote dispatcher always retains the legacy null response and never transfers
capture ownership to the phone. The mobile wrapper never sends the new parameter,
and its hook ignores any foreign reply metadata. There is no new RPC or provider work.

The QueryClient retains acknowledged local lifetime across component remounts. A
successful transition clears only incompatible diagnostic references from both source
states and cached Stats boards, preserving row identity, revisions, freshness, errors,
query data timestamps and UI selection. Future old/status-only frames and held Stats
answers cannot restore a retired link. Useful counts continue without a reference.
Only an accepted reference matching the acknowledged local capture restores linkage;
core handle-kind/owner/liveness validation remains strict. Failed saves publish no
context. A late success cannot establish native completion order and clears linkage
conservatively without overriding a later successful preference write.

Startup without a proven local acknowledgment remains unlinked. This bridge observes
this application's successful preference calls; it does not claim to observe capture
changes initiated by another native/remote caller. Such changes are still rejected by
strict core validation if a previously acknowledged context has become stale. No token
ordering, synthetic handle, retry or diagnostic lookup is used to recover correlation.
Task6C must establish its own local phone lifetime rather than use desktop metadata.

Stored StopFailure history may contain legacy ingestion-time fallback timestamps. A
candidate or retrospective diagnostic delta is now absent unless a retained independent
hook observation confirms that exact source timestamp. This deliberately sacrifices
unprovable historical timing (also after bounded correlation expiry) instead of treating
storage fallback as source time. Existing storage and the15-second classifier are
unchanged; no schema migration or additional query is introduced.

## Task 6C transcript and phone integration

`transcript_page::page` observes its actual bounded native read using the desktop
recorder, without changing reader/index policy. Native Transcript Read bytes are the
returned `page.bytes_read` (bytes held/read by that page path, including its cursor
work), not serialized/compressed wire size; streamed `bytes_scanned` is not silently
added. Rows are returned page messages; elapsed is that synchronous read section.
Failure leaves byte/row readings absent. Disabled capture takes the original read
path directly. The completed operation is retired and is not attached to the reply.

The real frontend page hook measures the existing awaited call (including transport
and decoding to the extent they happen inside that promise), not server-only time.
Follower state publications observe current residency and idle/follow states; explicit
hide transitions and actual dropped-page rows are observed without starting reads.
Evicted rows describe dropped page entries, not unique lifetime messages. The mounted
viewer observes its bounded window and input-array residency at React effect/commit.
`raf_proxy` is elapsed time to two browser animation callbacks after that commit;
it is neither compositor paint nor CPU/heap. An actual hidden transition permanently
censors the current sample even if rows stay identical across resume. Only a new
visible window commit starts a new sample. Unmounted/changed windows cancel pending
callbacks. Client observations are unlinked: no request ID, session path,
message ID, or retired host operation is fabricated. Exact duplicate suppression and
shared event quotas coalesce/refuse repeats with the foundation's loss accounting.

The companion owns a separate default-off `phone-measurement-prefs` key in its local
store and its own secure random epoch/phone-role recorder. Startup reads only that
local preference. Successful local persistence precedes enable/disable and returns
its own optional capture identity. Failed writes do not publish capture or preference
state. Phone Ready/Stats/transcript events use only local opt-in and local command
routing; any foreign receipt/scope/operation is omitted before submission. Ready
counts are observed, but the actual phone ConnectionBanner remains unmeasured, not
misreported as a displayed desktop footer. External preference origins retain the
previously documented lifecycle boundary.

Five specifically named companion commands (`get_phone_measurement_prefs`,
`set_phone_measurement_prefs`, `phone_measurement_status`, `record_phone_measurements`,
`export_phone_measurements`) are companion-only registrations using CLIENT_COMMANDS
and work without pairing. They are intentionally absent from the desktop command
surface and its companion mirror; desktop forwarding refuses them as unknown. The actual
“This phone” panel is mounted in General settings and on the unpaired screen. It
loads local status on mount/control/export, not a polling timer. Status/control
failure remains unavailable; disabled capture can still share retained history.
Phone report bytes and paths never cross JavaScript.

`export_recent_to` reuses the same writer, admission owner, five-second pre-streaming
barrier and immutable cutoff as full export. With rotation serialized, it makes one
bounded validation pass for payload bytes, one for suffix counts/ranges, and one to
stream selected whole records. It reserves 64 KiB for bounded header/trailer metadata,
rejects metadata exceeding that reservation, and checks the exact final file bytes
against 8 MiB. This may leave unused space; it never creates a full-history temporary
or materializes 128 MiB. Public full desktop export retains its prior cap. The phone
adapter reads only that bounded native file (8 MiB + one-byte overflow probe) for
native sharing and removes its private temporary after presentation completes.

Recent header adds `omitted_prefix: {records, oldest_wall_ms, newest_wall_ms}`. Counts
cover validated omitted records in writer order; malformed data stays in loss rather
than pretending to be counted. Wall ranges are minima/maxima, not cross-epoch clock
ordering. Header oldest/newest/records describe the selected suffix, and header/trailer
retain the actual barrier cutoff even when prefix records are omitted. Any omission
sets incomplete. Header build/platform/role describe the current exporter; retained
envelopes keep their own epoch/capture/role, and old epochs are not relabeled as current.

The native-only share bridge accepts closed TranscriptMarkdown/MeasurementJsonl kinds
with fixed `transcript.md` / `headstate-measurements.jsonl` names. Measurement MIME is
application/x-ndjson (iOS app UTI declaration and typed share item; Android intent).
Kind participates in the private cache hash. Both kinds share the existing 8-file,
32-MiB, 24-hour retention and 8-MiB per-file limits. Busy/cancel/presented outcomes keep
their original meaning; Android presented does not mean a destination saved the file.
A companion build is required; an older installed app honestly lacks these local
commands. No physical iPhone paint, sharing latency, background execution, memory,
or enterprise performance acceptance follows from the synthetic tests.
