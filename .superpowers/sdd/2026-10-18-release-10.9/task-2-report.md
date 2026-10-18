# Task 2 report: baseline/workload admission (#1856)

## Status

DONE. Baseline generation, comparison, and the actual native-connected runner now share exact workload, environment, sample, metric, and baseline-row admission contracts. The checked-in historical `scripts/enterprise/baseline.json` was not changed.

## Implementation

- Added immutable workload schema 1 in `scripts/enterprise/workload.mjs`:
  - exact roles `desktop` and `paired`;
  - seed, scenario, authored/reviewing/repository/member/history populations, cadence and schedule version;
  - exact quota profile `{id, hash}` so profile parameters cannot change behind a reused name;
  - unchanged legacy defaults (50 authored, 150 reviewing, 50 repositories, 50 members, 250 history), with the existing convergence modes accurately recording 236 reviewing;
  - exact compatibility assertions for every field.
- Added normalized runtime environment admission for platform, architecture, OS release, CPU, build class, strict mode, browser engine and browser version. `sourceRevision` is required provenance but intentionally excluded from compatibility equality.
- Defined baseline schema 2 with a workload descriptor and per-engine environments. Schema 1 rejects explicitly as historical/incompatible.
- Enforced exactly one desktop and one paired sample, finite nonnegative provider receipts, finite nonnegative required metrics independent of arbitrary budget keys, exact/finite budgets, unique rows, and exact matching rows for the selected engine/warm state.
- Preserved WebKit long-task absence as explicit unsupported capability with `null` values. Chromium requires finite nonnegative long-task count and duration.
- The generator validates every result before distribution construction and validates workload/environment consistency across all runs. Generated policies are validated before writing.
- The actual runner discovers the real browser version, then validates schema/rows, workload, and environment before creating any browser context/page or calling native `control('start')`. Rejections continue through the existing `finally` lifecycle and retain provider/native/accounting/result evidence.
- `result.workload` and `result.environment` expose the shared records for Tasks 3, 4, 7, and 8.
- Added `make enterprise-admission-probe` and documented schema/provenance/rejection behavior.

## Interface for subsequent tasks

- `normalizeWorkload(input)` returns a frozen schema-1 descriptor.
- `defaultWorkload` retains the legacy fixture contract.
- `assertCompatibleWorkload(actual, expected)` requires exact behavioral equality, including both quota profile identifier and hash.
- `normalizeEnvironment(input)` and `assertCompatibleEnvironment(actual, expected)` centralize host/runtime/build/browser admission while allowing source revision changes.
- `validateSamples(result)`, `validateBaseline(baseline)`, and `compareBaseline(result, baseline)` share the metric/row contract.
- Baseline schema 2 stores `environments[engine]`; source revisions live inside those environment records as provenance.

## TDD evidence

### RED

Command:

```text
node --test scripts/enterprise/workload.test.mjs scripts/enterprise/budgets.test.mjs
```

Expected failure before implementation:

```text
Error [ERR_MODULE_NOT_FOUND]: Cannot find module '.../scripts/enterprise/workload.mjs'
tests 2; pass 0; fail 2
```

The tests imported the required workload contract before it existed, proving the new behavior was not already present. The existing comparator test was replaced with strict role, metric, document, row, WebKit capability, workload, environment, and provenance cases before production implementation.

### GREEN

Focused command after implementation and final integration:

```text
node --test scripts/enterprise/workload.test.mjs scripts/enterprise/budgets.test.mjs
```

Result:

```text
tests 11; pass 11; fail 0
```

## Verification

Complete enterprise contracts, run once:

```text
make test-enterprise-contracts
```

Result: 111/111 Node contracts passed; the ignored native enterprise admission-accounting contract passed 1/1 (`3350 filtered out`).

Syntax and diff checks:

```text
node --check scripts/enterprise-run.mjs
node --check scripts/enterprise-baseline.mjs
git diff --check
```

All passed with no output.

Real native/browser admission probes used the already-built enterprise driver/UI:

```text
make enterprise-admission-probe OUT=/private/tmp/r109-task2-rejection-validated BASELINE=/private/tmp/r109-task2-incompatible.json ENGINE=chromium
make enterprise-admission-probe OUT=/private/tmp/r109-task2-environment-rejection BASELINE=/private/tmp/r109-task2-bad-environment.json ENGINE=chromium
make enterprise-admission-probe OUT=/private/tmp/r109-task2-malformed-row-rejection BASELINE=/private/tmp/r109-task2-malformed-row.json ENGINE=chromium
```

All three probe targets passed. Retained result artifacts:

- `/private/tmp/r109-task2-rejection-validated/result.json`: wrong seed rejected (`9001 !== 9002`).
- `/private/tmp/r109-task2-environment-rejection/result.json`: wrong CPU rejected.
- `/private/tmp/r109-task2-malformed-row-rejection/result.json`: empty row budgets rejected in preflight.

Every result records `admission.accepted=false`, `admission.stage=before-measured-work`, `providerReceipts=0`, `statsRetirement.commands=0`, `shutdown.clean=true`, `shutdown.telemetryLost=false`, and native `exit.code=0`.

## Files changed

- `scripts/enterprise/workload.mjs`
- `scripts/enterprise/workload.test.mjs`
- `scripts/enterprise/budgets.mjs`
- `scripts/enterprise/budgets.test.mjs`
- `scripts/enterprise-baseline.mjs`
- `scripts/enterprise-run.mjs`
- `scripts/enterprise/README.md`
- `Makefile`
- `.superpowers/sdd/2026-10-18-release-10.9/task-2-report.md`

## Self-review

- Confirmed `scripts/enterprise/baseline.json` is unchanged.
- Confirmed the controller-owned plan edit is neither modified by Task 2 nor staged.
- Confirmed no measurement batch was run; Task 9 owns current baseline measurement.
- Confirmed existing non-baseline runner modes retain their behavior; only convergence result provenance changes from the legacy 150 descriptor to its existing 236 population.
- Confirmed rejection occurs before page/context creation and before measured native start, while the established cleanup path remains responsible for evidence and shutdown.

## Concerns

The schema-2 measured baseline does not exist yet by design. `enterprise-compare` against the immutable checked-in schema-1 baseline now rejects explicitly until Task 9 writes a new versioned measured baseline. No speculative runtime flags or product changes were added.
