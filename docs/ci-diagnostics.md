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
