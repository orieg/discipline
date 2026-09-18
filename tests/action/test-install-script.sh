#!/usr/bin/env bash
# Exercises install.sh with positive and negative checksum controls against a fake release store.
set -euo pipefail

bin="${1:?path to discipline binary}"
scratch="${2:?scratch directory}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${here}/../.." && pwd)"

echo "== positive control: install.sh downloads, verifies checksum, and installs"
"${here}/fake-release.sh" "${bin}" "${scratch}/store" v0.0.0-test
inst_dir="${scratch}/inst-clean"
"${repo_root}/install.sh" --to "${inst_dir}" --version v0.0.0-test --download-url "file://${scratch}/store"
[ -x "${inst_dir}/discipline" ]
"${inst_dir}/discipline" --version

echo "== negative control: install.sh rejects tampered archive"
"${here}/fake-release.sh" "${bin}" "${scratch}/store" v0.0.0-tamper tamper
bad_inst_dir="${scratch}/inst-bad"
if "${repo_root}/install.sh" --to "${bad_inst_dir}" --version v0.0.0-tamper --download-url "file://${scratch}/store"; then
  echo "install.sh should have failed on checksum mismatch" >&2
  exit 1
fi
[ ! -f "${bad_inst_dir}/discipline" ]

echo "install.sh: both controls behaved"
