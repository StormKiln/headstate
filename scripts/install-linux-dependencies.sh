#!/usr/bin/env bash
# #1760: apt update exhausted the 45-minute platform job before tests began.
# #1776: a slow Azure mirror exhausted installation's budget while downloading.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo 'usage: install-linux-dependencies.sh PACKAGE [PACKAGE ...]' >&2
  exit 2
fi
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

# #713: remove only the unused Chrome list; universe must remain enabled.
sudo -n rm -f /etc/apt/sources.list.d/*google-chrome*
apt_options=(
  -o Acquire::Retries=2
  -o Acquire::http::Timeout=30
  -o Acquire::https::Timeout=30
  -o DPkg::Lock::Timeout=60
)
run_apt() {
  local label="$1" deadline="$2"
  shift 2
  # timeout must own root privileges to terminate apt and its children.
  python3 "$script_dir/ci-command.py" run "$label" -- \
    sudo -n env DEBIAN_FRONTEND=noninteractive LC_ALL=C \
    timeout --kill-after=5s "$deadline" apt-get "${apt_options[@]}" "$@"
}

# Reject incomplete indexes. No index/configuration failure is retried.
run_apt apt-update 300s -o APT::Update::Error-Mode=any update

# Download first: no dpkg changes can have happened if this attempt fails.
# Two 300s attempts share the original 600s network budget, and keep completed
# archives. Slow progress can defeat APT's per-connection timeout and prevent
# its ordinary mirror failover; only then retire the known Azure mirror.
apt_status=0
run_apt apt-download-primary 300s --download-only install -y "$@" || apt_status=$?
if [ "$apt_status" -ne 0 ]; then
  case "$apt_status" in
    124) ;; # GNU timeout's normal deadline; process cleanup completed.
    100)
      log="$RUNNER_TEMP/ci-diagnostics/apt-download-primary.log"
      # APT also uses 100 for bad configuration/dependencies: those must not
      # trigger source changes. Match fetch errors in the forced C locale.
      if ! grep -q '^E: Failed to fetch ' "$log" ||
         ! awk '/^E: / && !/^E: Failed to fetch / && !/^E: Unable to fetch some archives/ {bad=1} END {exit bad}' "$log"; then
        exit "$apt_status"
      fi
      ;;
    *) exit "$apt_status" ;; # Includes forced SIGKILL (137) and capture failures.
  esac

  # Hosted runners use this exact mirror-list pathname. Only recognize their
  # official entries; never rewrite custom sources or introduce another mirror.
  # Index paths and signed metadata remain unchanged: mirror+file identifies
  # the same archive, and APT validates cached/new .debs against its indexes.
  if ! python3 "$script_dir/ci-command.py" run apt-mirror-fallback -- \
    sudo -n python3 - /etc/apt/apt-mirrors.txt \
      "$RUNNER_TEMP/ci-diagnostics/apt-download-primary.json" <<'PY'
from pathlib import Path
import json
import sys

evidence = json.loads(Path(sys.argv[2]).read_text())
if evidence.get("captureIncomplete") or evidence.get("state") != "completed":
    print("Primary download evidence is incomplete; refusing recovery.")
    sys.exit(1)
path = Path(sys.argv[1])
azure = 'http://azure.archive.ubuntu.com/ubuntu/'
archive = 'https://archive.ubuntu.com/ubuntu/'
security = 'https://security.ubuntu.com/ubuntu/'
if not path.is_file():
    print('No hosted-runner mirror list; refusing to change sources.')
    sys.exit(1)
lines = path.read_text().splitlines(keepends=True)
entries = [(line, line.split()[0]) for line in lines
           if line.strip() and not line.lstrip().startswith('#')]
urls = {url for _, url in entries}
if not {azure, archive} <= urls or not urls <= {azure, archive, security}:
    print('Unrecognized mirror list; refusing to change sources.')
    sys.exit(1)
path.write_text(''.join(line for line in lines
                        if not line.strip() or line.lstrip().startswith('#')
                        or line.split()[0] != azure))
print('Retrying downloads with the existing official Ubuntu HTTPS mirrors.')
PY
  then
    exit "$apt_status"
  fi
  run_apt apt-download-fallback 300s --download-only install -y "$@"
fi

# Exactly one installation attempt, using only the validated cached archives.
# Never retry an interrupted dpkg transaction or silently omit missing packages.
run_apt apt-install 600s --no-download install -y "$@"
