# Bounded population discovery (#1885)

## Evidence and policy

Synthetic temporary files only; no normal home, credentials, provider or ignored
real-corpus probe. Host: macOS 26.6.2 (25G83), arm64, Rust 1.98.1; debug test
build with `enterprise-harness`. These are individual local observations, not a
supported-host timing envelope. Fixtures and original failed/successful logs remain
in `/private/tmp/r109-discovery-*.log` and the generated temporary directories.
Raw numerical records, including fixture locations and intermediate policies, are
in [the retained observations](../measurements/2026-10-18-discovery-observations.txt).

The original full-listing policy was measured **before changing it**:

| Fixture | Delay per certificate entry | Listing time | Certificate entries | Retained path/certificate bytes |
| --- | ---: | ---: | ---: | ---: |
| 1 project × 2,048 files | 0 | 47,442 µs | 2,049 | 362,381 |
| 32 projects × 64 files | 0 | 41,271 µs | 2,080 | 354,962 |
| 1 project × 2,048 files | 100 µs | 359,659 µs | 2,049 | 362,381 |

The baseline entry counter instruments certificate enumeration, not the separate
legacy walk. Baseline timing includes that walk and the existing readability/stat
probes, but not subsequent processing-lane certificate validation. A later
reference rerun still uses the unchanged legacy `corpus` policy; its retained raw
results show host variation. Both exact probe names now assert actual returned
transcript bytes `[0,0,0,0]` through the feature's thread-local read capture.

Initial implementation experiments used one transaction per observation. Those
cost 365.8 ms warm / 891.6 ms delayed for 2,048 files, and 1.36 seconds active over
three slices for 8,192 files. Batching at most 64 observations per transaction,
with all filesystem work outside the writer, reduced that cost to 99.8 ms /
732.3 ms and 443.9 ms respectively. A 16,384-unit ceiling still unnecessarily
spread 8,192 files over three 60-second population ticks. That ceiling was rejected.

The final policy uses **one second and at most 65,536 cooperative work units per
admission**, whichever is reached first. Enumeration, incremental ordered
certificate serialization, directory transitions and final stamp checks all
consume units. A 64-observation transaction batch is an internal bound, not the
number of entries allowed per 60-second production tick. The measured 8,192-file
fixture requires 32,789 units; rounding upward to 65,536 supplies almost 2× work
headroom. The one-second deadline supplies about 20% headroom over the final
831.7 ms delayed observation from the first final-policy run (the settled rerun
was 741.3 ms). This is scheduling policy, not a CI timing SLA.

| Final fixture | Delay per returned directory entry | Active time | Units | Directory entries | Admissions / cadence wait | Selected payload bytes |
| --- | ---: | ---: | ---: | ---: | --- | ---: |
| 2,048 files | 0 | 102,989 µs | 8,213 | 4,098 | 1 / 0 seconds | 522,489 |
| 2,048 files | 100 µs | 741,273 µs | 8,213 | 4,098 | 1 / 0 seconds | 522,489 |
| 8,192 files | 0 | 441,954 µs | 32,789 | 16,386 | 1 / 0 seconds | 2,089,209 |

The new pass includes durable inventory writes and a second names/types census;
it is **slower than legacy warm discovery**, not a general scan-speed improvement.
It prevents discovery from consuming the first accepted processing lane's turn,
retains useful observed candidates, and makes in-process continuation possible.
The unchanged production cadence remains 60 seconds. Larger/slower corpora can
require multiple ticks; completed acceptance is not promised within one tick.

Payload observations cover directory native-name/certificate strings and returned
scan session/source strings. They exclude diagnostics, tree nodes, SQL allocations, transient allocations,
allocator overhead, native RSS and recorder memory. They are not a constant-memory
claim or an application-memory measurement. The live certificate grows with one
directory and the returned scan grows with accepted/candidate inventory.

## Authority and lifecycle

Migration 48 is additive: epoch, directory and inventory tables plus ordered
pending/validation indexes. Existing metadata, FTS, usage and link evidence remain
intact. It does not infer a root from historical ledgers. **On first upgrade the
new inventory is empty**; old stored results stay visible, while sources enter the
processing inventory as bounded discovery observes them. Previously stored sources
are not promised immediate processing before their first observation. Once an
inventory exists, accepted sources receive the first rotated processing lane
before potentially expensive discovery. The migration/partial-first-discovery
regression verifies that old metadata, searchable FTS, index ledger, links and
link checkpoints survive an incomplete first census.

The fair lane rotation is persisted before discovery. The first lane consumes the
qualified accepted/candidate union. Discovery then advances its staging epoch;
remaining lanes consume the resulting union. After completed revalidation, the
first lane is revisited with its **original child deadline**, so totals and cleanup
can become complete without renewing its time budget. A stable fixture completes
metadata, usage and index through all four first-lane rotations, finishes links,
and reads zero unchanged transcript body bytes after its initial continuity audit.
The original expensive-usage fairness test's Complete assertion is preserved.

A live iterator is keyed by database/profile path, root and staging generation in
the population registry's single active discovery runtime. Switching that identity
discards the iterator conservatively. No ordinal, sorted-last-name or portable
seek token is persisted. Completed directory units and candidate rows are durable;
reopening retains candidates for processing, then replays an incomplete directory
from the beginning. Restarting during certificate validation restarts its validation
window while retaining completed first-pass units. Disappearing/replaced project
directories invalidate staging and retry a fresh epoch instead of blocking forever.
Unreadable file membership/diagnostics persist while healthy siblings continue.
Known failed directories are bypassed in favor of healthy pending units and then
retried in persisted failure-count order; one permanently unreadable project cannot
pin a recovered project. Directory failures keep the census partial. File failures
keep the processing scan partial even when membership validation completes.
Permission/database failures cannot replace accepted membership with an incomplete
census or permit downstream pruning.

Only a completed names/types validation pass plus supplemental final directory
stamp sweep can atomically advance accepted generation and remove older inventory
rows. Cancellation and staging-generation fences surround commits. Partial scans
remain qualified and cannot prune actual index/link continuation. Source body
publication still uses existing per-source before/after certificates and lane
generation/cancellation fences. Foreground/full-corpus callers retain their existing
full discovery implementation. Authoritative index/import consumers still check
the full source/directory certificate set, so their final validation may consume
that lane's remaining budget even after discovery itself has been sliced.

## Limits and verification

Cooperative checks cannot interrupt **one blocking OS/filesystem call**. A SQLite
statement/lock wait, atomic publication transaction, or materialization of the
accepted scan is also not a hard real-time operation. The one-second child deadline
bounds opportunities to begin discovery work; it is not a wall-clock maximum for
an admission. A parent cancellation can discard the current incomplete directory,
but previously committed candidate observations and accepted processing survive.

Repeated crashes/restarts inside the same large directory can repeatedly replay
its prefix and prevent acceptance. A stable process resumes its live iterator and
converges. The regression demonstrates the conservative restart limit, then
successful continuation. No lexicographic token pretends to solve this portability
constraint.

Multi-slice validation has a **longer observation window** than a short legacy
walk. Actual names/types comparison detects equal-timestamp membership changes
before a directory's second pass; the final stamp sweep rejects ordinary changes
after that pass. This is not an atomic filesystem snapshot. An adversarial
mutation/restoration after its certificate or final stamp check remains a portability
limit, with greater exposure when work spans ticks. Downstream authoritative
certificates are retained; they are not replaced by timestamps.

Deterministic tests assert structural work caps, actual accepted/candidate processing,
unknown totals while partial, unchanged-byte deltas, stable convergence/reopen,
add/rename/delete behavior, membership changes with equal timestamps, permission
errors, staging changes, cancellation during final validation and immediately before
commit, rollback/reopen, and authoritative-only actual index pruning. Migration
rollback/version/commit tests cover migration 48 through the full historical matrix.
The historical link migration fixture now constructs the actual version-46 schema
rather than pretending the newest schema is version 46.

Reproduce with `make test-enterprise-corpus-resources`,
`make test-enterprise-resources`, `make lint-enterprise-resources`, and
`make measure-enterprise-discovery`. The last target uses exact synthetic ignored
test names and retains its fixtures; it never runs ignored real-home probes.

## Review follow-up: unsupported native filenames

Unsupported non-UTF-8 transcript paths persist as SQLite BLOB identities containing
native encoded bytes; valid paths remain TEXT. SQLite equality/primary-key handling
keeps these distinct even when their lossy display would match a valid filename.
BLOB rows only produce unreadable-file diagnostics, never processing candidates or
reconstructed/opened source paths. The sole processing-scan reader handles BLOB
explicitly and rejects unexpected stored value types. Directory certificates still
contain the original native names and types. No schema migration is needed.

The Unix regression attempts a real raw-byte `0xff.jsonl` file, alongside a valid
replacement-character filename and a healthy later project. APFS rejected creation
with EILSEQ on the verification host; a guarded, thread-local, test-only entry-path
substitution supplies that single native name consistently to both enumeration
passes. The real-filesystem path remains for Linux CI, which was not run locally.
Repeated admissions and reopen preserve accepted evidence, retain qualified status,
validate membership, and actually index all three healthy sources.

The all-lane rotation assertion now inspects the production link lane's actual
returned result through a test-only field. It performs no foreground scan or extra
link refresh after population. Metadata/usage/index Complete assertions and zero
unchanged body-read deltas are retained.
