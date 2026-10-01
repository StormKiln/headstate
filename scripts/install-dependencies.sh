#!/usr/bin/env bash
# Existing immutable-install retry policy from the setup action; #1361 now captures it.
set -e
# LOAD-BEARING, and not a habit: `yarn ... | tee` reports TEE's status
# without it, so a failing install exits 0, the error is never printed,
# and the job goes GREEN on a broken tree. Measured by removing this
# line against a stub that exits 1: exit=0, zero error annotations.
set -o pipefail
attempts=5
log="$(mktemp)"

# Does this output look like a DEPENDENCY problem rather than a
# network one? Inverted on purpose (#932).
#
# The first version of this rule matched a list of network
# signatures -- ECONNRESET, ETIMEDOUT, EAI_AGAIN, "tunneling socket
# could not be established" -- and retried only on those. The flake
# that burned a release base on 2026-09-16 was
# `write ECANCELED ... Canceled because of SSL destruction`, a fifth
# spelling none of them matched. A list of network errors is a list
# of the ones we have already seen.
#
# The set of ways a LOCKFILE can be wrong is small, closed and
# yarn's own vocabulary: it prints a YN#### code for each. So the
# question asked here is "is this the tree's fault", and anything
# else -- including a spelling nobody has met yet -- is treated as
# the network and retried. An unrecognised failure costs a few
# minutes of backoff; an unrecognised NETWORK failure that is not
# retried costs a release tag, because the gate reads every
# check-run attempt and a failed one is permanent (#1048).
looks_like_the_tree() {
  grep -qE \
    'YN0028|YN0016|YN0035|not found in the registry|lockfile would have been (created|modified)|doesn'"'"'t provide|isn'"'"'t supported by any available (fetcher|resolver)' \
    "$1"
}

for attempt in $(seq 1 "$attempts"); do
  if yarn install --immutable 2>&1 | tee "$log"; then
    if [ "$attempt" -gt 1 ]; then
      echo "::notice::yarn install succeeded on attempt $attempt of $attempts"
    fi
    exit 0
  fi
  if looks_like_the_tree "$log"; then
    # Fast failure for the case a retry cannot fix. A broken
    # lockfile is broken on every attempt, and waiting four more
    # minutes to say so helps nobody.
    tail_text="$(tail -n 20 "$log" | tr '\n' '|' | sed 's/|/ | /g')"
    echo "::error::yarn install failed on attempt $attempt and the output \
names a dependency problem, which a retry cannot fix. Last output: $tail_text"
    exit 1
  fi
  if [ "$attempt" -lt "$attempts" ]; then
    # 15s, 45s, 90s, 150s -- 300 seconds in total, against the 45s
    # that #932 measured as too short to outlast a proxy outage.
    #
    # An explicit table rather than a formula. The first version
    # here was `attempt * attempt * 15 - (attempt - 1) * 15`, whose
    # comment claimed this sequence and whose arithmetic produced
    # 15/45/105/195 = 360s. A reader cannot check a formula at a
    # glance; they can check four numbers.
    case "$attempt" in
      1) backoff=15 ;;
      2) backoff=45 ;;
      3) backoff=90 ;;
      *) backoff=150 ;;
    esac
    echo "::warning::yarn install failed (attempt $attempt/$attempts); \
the output does not name a dependency problem, so this is being treated \
as a network failure; retrying in ${backoff}s"
    sleep "$backoff"
  fi
done
# The last 20 lines, flattened to one annotation line: GitHub renders a
# multi-line annotation as its first line only, so newlines are replaced
# rather than lost. The full output is still above in the step log.
tail_text="$(tail -n 20 "$log" | tr '\n' '|' | sed 's/|/ | /g')"
echo "::error::yarn install failed on all $attempts attempts. Last output: $tail_text"
exit 1
