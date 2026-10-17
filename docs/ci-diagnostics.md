# Rust CI diagnostic evidence

The Rust job retains stage output and atomic status checkpoints because the
obsolete #1735 timeout's hosted log was unavailable. The old cause is unknown.
These records help distinguish build cost, test execution, command failure and
cancellation. They do not prove a timeout was a deadlock.

The job still has a **30-minute whole-job limit**. Initial step-up and desktop
Cargo tests and enterprise contracts are unchanged. Race checking still runs
`cargo test --lib -- --test-threads=8` three times, stopping at the first failure.
CI and `make test-race` use `scripts/test-rust-ci.py`. No retries or inactivity
timeouts were added. A command's nonzero status is preserved by the runner;
GNU Make itself may report its standard recipe-failure status to its caller.

The `command-diagnostics-test-rust-<OS>` artifact contains distinct `.log` and
`.json` files for `rust-stepup`, `rust-desktop`, `rust-enterprise-contracts` and
`rust-race-1`, `rust-race-2`, `rust-race-3`. The existing supervisor streams the
original combined stdout/stderr while writing the raw log. JSON records UTC
start/checkpoint/finish times, monotonic elapsed time, child PID, output bytes,
exit status and cancellation. Rust checkpoints update at most once per second;
console heartbeats remain every 30 seconds, including during quiet healthy work.
Local invocations without `RUNNER_TEMP` print a retained temporary directory.

Opt-in `--rust` parsing adds `rustProgress`:

- `phase` observes build messages, the test banner or a suite summary.
- `lastCompletedTest` records the latest recognized terminal test line, its
  result and **observation** time. It is not the currently running test.
- `reportedSlowTests` contains up to 32 identities explicitly reported by libtest
  as running for over 60 seconds. A later terminal line clears that identity;
  a new suite or suite summary clears the set.
- `activeTestVisibilityComplete` is always false. Stable libtest text does not
  announce every start. An empty slow set means no currently retained slow
  reports, not that no tests are running. Observation times are not start times
  or test durations.
- Parsing holds at most 8 KiB of a line and recognized test names are at most 1,024 characters.
  Oversized lines are discarded through their newline; overflow of the line or
  32-entry slow set sets `truncated`. Raw logs retain the original output.

ANSI styling is removed only for parsing. The progress metadata is bounded;
raw command logs retain the existing supervisor's full-output behavior and
must be treated with the same access policy as hosted command output. No
command arguments or environment values are added to the metadata.

A final `completed` record describes command completion, **not success**: inspect
`exitCode` and `captureIncomplete`. Spawn failure is 127. Cancellation records
128 plus the signal and stops the owned process group. Capture/persistence
failure cannot make a failing command pass; incomplete capture causes nonzero
status even if the command succeeded. A persistence error stops owned work.
A remaining `running` checkpoint, missing final record or absent artifact means
execution evidence is incomplete. Never infer success from those states.

The always-run upload is best effort: runner loss or hard termination can prevent
upload or final checkpointing. Start/end notices provide a second breadcrumb,
not guaranteed recovery. Compare stage and whole-job times before attributing a
timeout. All-attempt release checks remain mandatory; a retry does not erase a
failed check attempt on a commit.

Run the fake-process regression suites with:

```
python3 scripts/ci-command.test.py
python3 scripts/test-rust-ci.test.py
```

They exercise real subprocesses, exits, cancellation, retained pipes, capture and
persistence failures, quiet work, parsing bounds, distinct race iterations and
fail-fast behavior. They do not invoke Cargo or establish native test success.

## Frontend native crash evidence (#1754)

The hosted Node SIGSEGV in run `37621024762` exited 139 before the suite
finished. Its four existing artifacts establish an incomplete run, not a
specific Node/V8 defect, OOM, or test assertion failure. The cause remains open.

The frontend wrapper still runs the same discovery and full-suite commands,
then requires the existing complete JSON report. Its temporary FIFO lets bash
retain the actual Node PID and wait for both Node and log capture. A native
failure remains a failure even if evidence collection fails. No retry, pool,
worker, timeout, heap, or assertion settings are changed.

After a reportable native signal, `collect-frontend-crash.py` polls macOS's user
and system `Library/Logs/DiagnosticReports` directories for at most ten seconds.
Only a fresh `node*.ips` crash report with the exact launched child PID, process
name `node`, and capture time within the command window qualifies. The current
`vmThreads` workers share that process. Discovery and execution have separate
`native-discovery.json` and `native-vitest.json` artifacts. Old evidence is
removed before each run. An ordinary exit or SIGKILL does not imply that an
Apple native crash report exists and does not start this collection.

Collection reads at most 1,024 directory entries and 16 fresh candidates per
poll, with a 2 MiB input-file ceiling and 128 retained frames from the faulting
thread. Symlinks and non-regular files are rejected. Output contains only the
PID, exception type/signal, faulting-thread index, numeric image/frame offsets,
binary UUID/architecture and bounded native symbols. Paths, arbitrary image or
thread names, registers, environment, command line, incident/device identifiers
and raw report contents are never copied into these artifacts. Unsupported
symbols are omitted; `symbolizedFrames` reports how many remain. Frame bounds and
omissions are explicit through `truncated`. This supplements the existing
Vitest/Node artifacts; it does not broaden their upload to any OS log directory.

`status: unavailable` is an absence of evidence, not a clean native stack.
Its reason distinguishes missing/unreadable reports, directory access, scan
bounds and unsupported platforms. Fixed numeric `rejectionCounts` identify
selection failures without copying report values or paths; `scanCounts` counts
entries and candidates. These describe only the final poll (`countsScope`), so
repeated observations are not presented as distinct files. Numeric `window`
bounds and the bounded poll count identify the collection window. OS report generation or runner shutdown can
prevent capture entirely. A captured unsymbolized image offset can still be
useful but does not identify a cause by itself.

Run synthetic fixtures with `python3 scripts/collect-frontend-crash.test.py`.
The separate **Native crash evidence acceptance** manual workflow compiles a
tiny synthetic program named `node`, disables core dumps, and causes a native
fault with no user/application data. It requires a matching symbolized stack;
unavailable evidence fails that acceptance run. Only sanitized JSON is uploaded.
This optional experiment is deliberately outside the product CI prerequisites.
Run it on a diagnostic branch/commit, observe every attempt, and do not treat a
rerun as erasing a failed release check. The equivalent local macOS command is:

```
python3 scripts/collect-frontend-crash.test.py --native-smoke /tmp/native-evidence.json
```

The parser follows Apple's [IPS format documentation](https://developer.apple.com/documentation/xcode/interpreting-the-json-format-of-a-crash-report).
Passing this synthetic test verifies the collection path, not a correction for
the intermittent frontend crash. A future real crash must identify a failing
native path before a runtime or pool change is justified.


### 10.6 hosted recurrence

Merge-queue run `39037370433`, candidate `b6b03d1cf43c1496405925b61b90d18bb4943430`,
terminated Node PID 4774 with SIGSEGV/139 at 2026-10-17 13:42:36 UTC.
329 of 342 discovered test files printed completed results. No assertion failure
or complete result JSON was captured. The collector returned `unavailable` with
`no_matching_report`; no native stack was obtained. This does not establish OOM,
a particular test defect, or failure of the OS to generate a report: selection
can also reject a report. #1887 adds bounded numeric selection evidence to
separate these cases without retaining raw crash data.

The same product branch passed all ten PR checks in run `39035784098`.
The failed merge candidate remains ineligible for release; a later passing run
cannot erase its failed attempt. No runtime, pool, retry, heap, timeout or test
assertion changes are justified by this recurrence. Root-cause issue #1754 stays
open under sustained-reliability epic #1851 after the diagnostic improvement.

The separate hosted synthetic acceptance run `39037909055` passed on the
already-disqualified diagnostic commit: its sanitized artifact contains a
matching synthetic SIGSEGV with two symbolized frames. This proves that the
hosted OS/report-selector path can work for the small compiled fixture. It does
not reproduce pinned Node/Vitest or explain why the real process yielded no
selected report. The next discriminating experiment belongs to #1754.
