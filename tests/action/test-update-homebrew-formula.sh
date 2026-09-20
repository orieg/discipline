#!/usr/bin/env bash
# Exercises update_homebrew_formula.py with positive and negative controls.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "${here}/../.." && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf "${scratch}"' EXIT

echo "== positive control: generates formula with release checksums"
cat << 'EOF' > "${scratch}/SHA256SUMS"
1111111111111111111111111111111111111111111111111111111111111111  discipline-aarch64-apple-darwin.tar.gz
2222222222222222222222222222222222222222222222222222222222222222  discipline-x86_64-apple-darwin.tar.gz
3333333333333333333333333333333333333333333333333333333333333333  discipline-aarch64-unknown-linux-musl.tar.gz
4444444444444444444444444444444444444444444444444444444444444444  discipline-x86_64-unknown-linux-musl.tar.gz
EOF

out_formula="${scratch}/discipline.rb"
python3 "${repo_root}/scripts/update_homebrew_formula.py" \
  --version 1.2.3 \
  --checksums "${scratch}/SHA256SUMS" \
  --output "${out_formula}"

grep -q '1111111111111111111111111111111111111111111111111111111111111111' "${out_formula}"
grep -q '2222222222222222222222222222222222222222222222222222222222222222' "${out_formula}"
grep -q '3333333333333333333333333333333333333333333333333333333333333333' "${out_formula}"
grep -q '4444444444444444444444444444444444444444444444444444444444444444' "${out_formula}"
grep -q 'v1.2.3' "${out_formula}"

if command -v ruby >/dev/null 2>&1; then
  ruby -c "${out_formula}" >/dev/null
fi

echo "== negative control: missing checksums file exits non-zero"
if python3 "${repo_root}/scripts/update_homebrew_formula.py" \
  --version 1.2.3 \
  --checksums "${scratch}/nonexistent_sums" \
  --output "${scratch}/bad.rb" 2>/dev/null; then
  echo "Expected missing checksums to fail" >&2
  exit 1
fi

echo "== positive control: push-to-tap clones, commits, and pushes formula"
tap_bare="${scratch}/bare-tap.git"
git init --bare "${tap_bare}"
init_clone="${scratch}/init-clone"
git clone "${tap_bare}" "${init_clone}"
mkdir -p "${init_clone}/Formula"
echo "# placeholder" > "${init_clone}/Formula/discipline.rb"
git -C "${init_clone}" config user.name "tester"
git -C "${init_clone}" config user.email "test@example.com"
git -C "${init_clone}" add Formula/discipline.rb
git -C "${init_clone}" commit -m "init tap"
git -C "${init_clone}" push origin HEAD

python3 "${repo_root}/scripts/update_homebrew_formula.py" \
  --version 1.2.3 \
  --checksums "${scratch}/SHA256SUMS" \
  --push-to-tap "${tap_bare}" \
  --tap-token "mock-token" \
  --output "${scratch}/test-discipline.rb"

verify_clone="${scratch}/verify-clone"
git clone "${tap_bare}" "${verify_clone}"
grep -q '1111111111111111111111111111111111111111111111111111111111111111' "${verify_clone}/Formula/discipline.rb"
grep -q 'v1.2.3' "${verify_clone}/Formula/discipline.rb"

echo "update_homebrew_formula: all controls behaved"
