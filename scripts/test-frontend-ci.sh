#!/usr/bin/env bash
# #1611: the hosted run exited before Vitest printed its summary. Capture
# evidence without changing its pool, worker count, timeouts, or assertions.
set -euo pipefail

diagnostics="${RUNNER_TEMP:?RUNNER_TEMP must be set}/frontend-diagnostics"
mkdir -p "$diagnostics"
export NODE_OPTIONS="${NODE_OPTIONS:-} --trace-exit --report-on-fatalerror --report-uncaught-exception --report-exclude-env --report-exclude-network --report-directory=\"$diagnostics\""

# A FIFO gives both processes waitable PIDs on macOS's bash 3.2, where a
# process-substitution tee cannot reliably be waited. Node remains the direct child.
run_node() {
  local stage="$1" started ended node_pid tee_pid node_status tee_status pipe_dir
  shift
  pipe_dir=$(mktemp -d "$RUNNER_TEMP/frontend-pipe.XXXXXX") || return 1
  if ! mkfifo "$pipe_dir/output"; then rmdir "$pipe_dir"; return 1; fi
  started=$(date +%s)
  tee "$diagnostics/$stage.log" < "$pipe_dir/output" &
  tee_pid=$!
  node "$@" > "$pipe_dir/output" 2>&1 &
  node_pid=$!
  wait "$node_pid"
  node_status=$?
  ended=$(date +%s)
  wait "$tee_pid"
  tee_status=$?
  rm -f "$pipe_dir/output"
  rmdir "$pipe_dir"
  case "$node_status" in
    132|133|134|136|138|139)
      # Collection is bounded and cannot replace the original native failure.
      python3 scripts/collect-frontend-crash.py --pid "$node_pid" \
        --started "$started" --ended "$ended" --output "$diagnostics/native-$stage.json" || \
        echo '::warning::Native evidence collection failed; the original test failure is preserved.'
      ;;
  esac
  if [[ "$node_status" -ne 0 ]]; then return "$node_status"; fi
  return "$tee_status"
}

# A stale report must never certify a later early exit.
rm -f "$diagnostics/vitest.json" "$diagnostics/files.json" "$diagnostics/native-discovery.json" "$diagnostics/native-vitest.json"
set +e
run_node discovery ./node_modules/vitest/vitest.mjs list --filesOnly --json="$diagnostics/files.json"
status=$?
set -e
if [[ "$status" -ne 0 ]]; then
  printf '%s\n' "$status" > "$diagnostics/exit-code.txt"
  exit "$status"
fi

set +e
# Yarn maps native signals it does not recognize (including SIGSEGV) to
# exit 1 without a message. Invoke the pinned Node and local Vitest entry
# directly so bash retains 128 + signal in the log and exit-code artifact.
# --trace-exit distinguishes an explicit process.exit() from a native crash.
run_node vitest ./node_modules/vitest/vitest.mjs run --logHeapUsage --reporter=default --reporter=json \
  --outputFile.json="$diagnostics/vitest.json"
status=$?
set -e
printf '%s\n' "$status" > "$diagnostics/exit-code.txt"
if [[ ! -s "$diagnostics/vitest.json" ]]; then
  echo '::warning::Vitest did not write its JSON result; inspect frontend diagnostics for an incomplete run.'
fi
if [[ "$status" -ne 0 ]]; then
  exit "$status"
fi
python3 scripts/check-frontend-report.py "$diagnostics/files.json" "$diagnostics/vitest.json"
