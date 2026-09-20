#!/usr/bin/env bash
# tests/action/test-check-major-tag.sh
# Exercises check-major-tag.sh with positive and negative controls.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="${here}/check-major-tag.sh"
scratch="$(mktemp -d)"
trap 'rm -rf "${scratch}"' EXIT

cd "${scratch}"
git init -b main
git config user.name "test"
git config user.email "test@example.invalid"
git config commit.gpgsign false
git config tag.gpgsign false

echo "init" > README.md
git add README.md
git commit -m "chore: initial"
c1="$(git rev-parse HEAD)"

# Tag v0.1.0 and floating v0 pointing to c1
git tag v0.1.0
git tag v0

echo "== positive control: auto-discovery passes when v0 matches latest release"
"${script}"

echo "== positive control: explicit args pass when pointing to expected commit"
"${script}" v0 "${c1}"

echo "second commit" >> README.md
git add README.md
git commit -m "feat: new feature"
c2="$(git rev-parse HEAD)"
git tag v0.2.0

echo "== negative control: auto-discovery fails when major tag drifts behind latest release"
if "${script}" 2>/dev/null; then
  echo "Expected check-major-tag.sh to fail on drift, but it passed" >&2
  exit 1
fi

echo "== negative control: explicit args fail when major tag does not deref to expected commit"
if "${script}" v0 "${c2}" 2>/dev/null; then
  echo "Expected check-major-tag.sh to fail when v0 != c2, but it passed" >&2
  exit 1
fi

echo "== positive control: moving v0 to latest release restores pass"
git tag -f v0 "${c2}"
"${script}"
"${script}" v0 "${c2}"

echo "ALL CONTROLS PASSED"
