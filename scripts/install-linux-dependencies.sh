#!/usr/bin/env bash
# #1760: apt update exhausted the 45-minute platform job before tests began.
# Keep transport retries finite and retain output/status through ci-command.py.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo 'usage: install-linux-dependencies.sh PACKAGE [PACKAGE ...]' >&2
  exit 2
fi
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

# #713: the unused Chrome index has served corrupt data. Remove only Chrome:
# disabling all sourceparts also disables Ubuntu universe and our GTK packages.
sudo -n rm -f /etc/apt/sources.list.d/*google-chrome*

apt_options=(
  -o Acquire::Retries=2
  -o Acquire::http::Timeout=30
  -o Acquire::https::Timeout=30
  -o DPkg::Lock::Timeout=60
)
# timeout runs INSIDE sudo so it can terminate apt and its privileged children.
# GNU timeout returns 124 on deadline (137 if forced); the wrapper preserves it.
# Error-Mode=any rejects incomplete index refreshes instead of using stale lists.
python3 "$script_dir/ci-command.py" run apt-update -- \
  sudo -n timeout --kill-after=5s 300s \
  apt-get "${apt_options[@]}" -o APT::Update::Error-Mode=any update
python3 "$script_dir/ci-command.py" run apt-install -- \
  sudo -n timeout --kill-after=5s 600s \
  apt-get "${apt_options[@]}" install -y "$@"
