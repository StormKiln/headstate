#!/usr/bin/env bash
# #1611: the hosted run exited before Vitest printed its summary. Capture
# evidence without changing its pool, worker count, timeouts, or assertions.
set -euo pipefail

diagnostics="${RUNNER_TEMP:?RUNNER_TEMP must be set}/frontend-diagnostics"
mkdir -p "$diagnostics"
export NODE_OPTIONS="${NODE_OPTIONS:-} --trace-exit --report-on-fatalerror --report-uncaught-exception --report-exclude-env --report-exclude-network --report-directory=\"$diagnostics\""

set +e
# Yarn maps native signals it does not recognize (including SIGSEGV) to
# exit 1 without a message. Invoke the pinned Node and local Vitest entry
# directly so bash retains 128 + signal in the log and exit-code artifact.
# --trace-exit distinguishes an explicit process.exit() from a native crash.
node ./node_modules/vitest/vitest.mjs run --logHeapUsage --reporter=default --reporter=json \
  --outputFile.json="$diagnostics/vitest.json" 2>&1 | tee "$diagnostics/vitest.log"
status=$?
set -e
printf '%s\n' "$status" > "$diagnostics/exit-code.txt"
if [[ ! -s "$diagnostics/vitest.json" ]]; then
  echo '::warning::Vitest did not write its JSON result; inspect frontend diagnostics for an incomplete run.'
fi
exit "$status"
