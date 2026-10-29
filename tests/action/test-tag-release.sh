#!/usr/bin/env bash
# tests/action/test-tag-release.sh
# Exercises scripts/tag-release.sh with positive and negative controls against a
# local bare repository standing in for origin.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="${here}/../../scripts/tag-release.sh"
scratch="$(mktemp -d)"
trap 'gpgconf --kill gpg-agent >/dev/null 2>&1 || true; rm -rf "${scratch}"' EXIT
export GNUPGHOME="${scratch}/gnupg"
mkdir -p "${GNUPGHOME}"
chmod 0700 "${GNUPGHOME}"
gpg --batch --passphrase '' --quick-generate-key "test <test@example.invalid>" default default never >/dev/null 2>&1
keyid="$(gpg --with-colons --list-secret-keys | awk -F: '$1=="sec"{print $5; exit}')"

mkdir -p "${scratch}/bin"
cat <<'EOF' > "${scratch}/bin/gh"
#!/usr/bin/env bash
if [ "${GH_MOCK_FAIL:-0}" = "1" ]; then
  echo "error: simulated gh failure" >&2
  exit 1
fi
jq_expr=""
workflow=""
branch=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --jq) jq_expr="$2"; shift 2 ;;
    --workflow) workflow="$2"; shift 2 ;;
    --branch) branch="$2"; shift 2 ;;
    *) shift ;;
  esac
done
# The Release run of a candidate tag: its commit is the tag's, unless overridden.
if [ "${workflow}" = "release.yml" ]; then
  head="${GH_MOCK_RELEASE_SHA:-$(git rev-parse -q --verify "refs/tags/${branch}^{commit}" || true)}"
  case "${GH_MOCK_RELEASE_STATUS:-success}" in
    failure) json="[{\"status\":\"completed\",\"conclusion\":\"failure\",\"headSha\":\"${head}\"}]" ;;
    empty) json='[]' ;;
    *) json="[{\"status\":\"completed\",\"conclusion\":\"success\",\"headSha\":\"${head}\"}]" ;;
  esac
  if [ -n "${jq_expr}" ]; then echo "${json}" | jq -r "${jq_expr}"; else echo "${json}"; fi
  exit 0
fi
case "${GH_MOCK_STATUS:-success}" in
  failure) json='[{"status":"completed","conclusion":"failure"}]' ;;
  in_progress) json='[{"status":"in_progress","conclusion":""}]' ;;
  empty) json='[]' ;;
  success) json='[{"status":"completed","conclusion":"success"}]' ;;
  *) json="${GH_MOCK_STATUS}" ;;
esac
if [ -n "${jq_expr}" ]; then
  echo "${json}" | jq -r "${jq_expr}"
else
  echo "${json}"
fi
EOF
chmod +x "${scratch}/bin/gh"
export PATH="${scratch}/bin:${PATH}"

git init -q --bare -b main "${scratch}/origin.git"
git init -q -b main "${scratch}/work"
cd "${scratch}/work"
git config user.name "test"
git config user.email "test@example.invalid"
git config user.signingkey "${keyid}"
git config commit.gpgsign false
git config tag.gpgsign false
git remote add origin "${scratch}/origin.git"

set_version() {
  printf '[package]\nname = "discipline"\nversion = "%s"\n' "$1" > Cargo.toml
  printf '[[package]]\nname = "discipline"\nversion = "%s"\n' "${2:-$1}" > Cargo.lock
  git add Cargo.toml Cargo.lock
  git commit -q -m "chore(release): bump version to $1"
}

# A candidate tag, pushed, as the release pipeline would have run it.
candidate() {
  git tag -a -m "v$1" "v$1" "${2:-HEAD}"
  git push -q origin "refs/tags/v$1"
}

set_version 0.1.0
old="$(git rev-parse HEAD)"
set_version 0.2.0-rc.1
rc="$(git rev-parse HEAD)"
set_version 0.2.0
bump="$(git rev-parse HEAD)"
git push -q origin main

# A stale side branch carrying the old version, as a leftover worktree would.
git checkout -q -b stale "${old}"
echo stale > stale.txt
git add stale.txt
git commit -q -m "fix: unrelated"

expect_fail() {
  local name="$1" pattern="$2"; shift 2
  local out
  if out="$("${script}" --unsigned "$@" 2>&1)"; then
    echo "FAIL: ${name}: expected failure, got: ${out}"; exit 1
  fi
  printf '%s\n' "${out}" | grep -q -- "${pattern}" || {
    echo "FAIL: ${name}: expected '${pattern}', got: ${out}"; exit 1; }
  if git rev-parse --verify --quiet "refs/tags/v0.2.0" > /dev/null; then
    echo "FAIL: ${name}: left a local tag behind"; exit 1
  fi
  echo "ok: ${name}"
}

expect_fail_direct() {
  local name="$1" pattern="$2"; shift 2
  local out
  if out="$("${script}" "$@" 2>&1)"; then
    echo "FAIL: ${name}: expected failure, got: ${out}"; exit 1
  fi
  printf '%s\n' "${out}" | grep -q -- "${pattern}" || {
    echo "FAIL: ${name}: expected '${pattern}', got: ${out}"; exit 1; }
  echo "ok: ${name}"
}

echo "== negative controls (HEAD is a stale branch at 0.1.0)"
expect_fail "empty --commit is refused, never HEAD" "--commit is empty" 0.2.0 --commit ""
expect_fail "HEAD off main is refused" "is not on origin/main" 0.2.0 --commit HEAD
expect_fail "version mismatch with Cargo.toml" "Cargo.toml at" 0.3.0 --commit "${bump}"
expect_fail "no bump commit on main is refused" "cannot find version-bump commit for 0.3.0" 0.3.0
expect_fail "malformed version" "is not X.Y.Z" 0.2
expect_fail "unknown commit" "does not name a commit" 0.2.0 --commit deadbeef
expect_fail_direct "--unsigned --push is refused" "--unsigned cannot be combined with --push" 0.2.0 --commit "${bump}" --unsigned --push

git checkout -q main
set_version 0.2.1 0.2.0
git push -q origin main
expect_fail "Cargo.lock out of step with Cargo.toml" "Cargo.lock at" 0.2.1
git checkout -q stale

echo "== negative controls: CI status check"
GH_MOCK_STATUS=failure expect_fail "failing CI status is refused" "CI status for commit" 0.2.0 --commit "${bump}"
GH_MOCK_STATUS=in_progress expect_fail "in-progress CI status is refused" "CI status for commit" 0.2.0 --commit "${bump}"
GH_MOCK_STATUS=empty expect_fail "missing CI runs refused" "no CI runs found for commit" 0.2.0 --commit "${bump}"
GH_MOCK_FAIL=1 expect_fail "unreadable CI status is refused" "cannot read CI status" 0.2.0 --commit "${bump}"

echo "== negative controls: a stable release needs a passing candidate on the same code"
expect_fail "no candidate on the remote is refused" "no release candidate v0.2.0-rc.N on origin" 0.2.0 --commit "${bump}"
candidate 0.2.0-rc.1 "${rc}"
GH_MOCK_RELEASE_STATUS=failure expect_fail "a failed candidate run is refused" "Release run of v0.2.0-rc.1 is" 0.2.0 --commit "${bump}"
GH_MOCK_RELEASE_STATUS=empty expect_fail "a candidate never run is refused" "no Release run found for v0.2.0-rc.1" 0.2.0 --commit "${bump}"
GH_MOCK_RELEASE_SHA="${old}" expect_fail "a run of another commit is refused" "Release run of v0.2.0-rc.1 is" 0.2.0 --commit "${bump}"

git checkout -q main
set_version 0.4.0-rc.1
candidate 0.4.0-rc.1
echo 'fn main() {}' > main.rs
git add main.rs
git commit -q -m "fix: code after the candidate"
set_version 0.4.0
git push -q origin main
expect_fail "code changed after the candidate is refused" "changed since: main.rs" 0.4.0 --commit HEAD
set_version 0.4.0-rc.2
candidate 0.4.0-rc.2
printf '[[package]]\nname = "discipline"\nversion = "0.4.0-rc.2"\n\n[[package]]\nname = "zlib"\nversion = "1.1.31"\n' > Cargo.lock
git add Cargo.lock
git commit -q -m "fix(deps): raise zlib"
printf '[package]\nname = "discipline"\nversion = "0.4.0"\n' > Cargo.toml
sed -i.bak 's/^version = "0.4.0-rc.2"$/version = "0.4.0"/' Cargo.lock && rm Cargo.lock.bak
git add Cargo.toml Cargo.lock
git commit -q -m "chore(release): bump version to 0.4.0"
git push -q origin main
expect_fail "a dependency changed after the candidate is refused" "changed since: Cargo.lock" 0.4.0 --commit HEAD
git checkout -q -b side HEAD~3
set_version 0.4.0-rc.3
candidate 0.4.0-rc.3
git checkout -q main
expect_fail "a candidate off the release's history is refused" "is not an ancestor of" 0.4.0 --commit HEAD
git checkout -q stale

echo "== positive control: an explicit commit on main is tagged with signature and pushed"
"${script}" 0.2.0 --commit "${bump}" --push
[ "$(git rev-parse 'v0.2.0^{commit}')" = "${bump}" ] || { echo "FAIL: local tag target"; exit 1; }
[ "$(git -C "${scratch}/origin.git" rev-parse 'v0.2.0^{commit}')" = "${bump}" ] ||
  { echo "FAIL: pushed tag target"; exit 1; }
[ "$(git cat-file -t v0.2.0)" = "tag" ] || { echo "FAIL: tag is not annotated"; exit 1; }
git tag -v v0.2.0 >/dev/null 2>&1 || { echo "FAIL: tag signature failed verification"; exit 1; }

echo "== negative controls: an existing tag is refused"
expect_existing() {
  local name="$1" pattern="$2"
  local out
  if out="$("${script}" --unsigned 0.2.0 --commit "${bump}" 2>&1)"; then
    echo "FAIL: ${name}: expected failure, got: ${out}"; exit 1
  fi
  printf '%s\n' "${out}" | grep -q -- "${pattern}" || {
    echo "FAIL: ${name}: expected '${pattern}', got: ${out}"; exit 1; }
  echo "ok: ${name}"
}
expect_existing "tag exists locally" "already exists locally"
git tag -d v0.2.0 > /dev/null
expect_existing "tag exists on the remote" "already exists on origin"

echo "== positive control: default commit is version-bump commit, not tip of main; nothing pushed"
git checkout -q main
set_version 0.3.0-rc.1
candidate 0.3.0-rc.1
set_version 0.3.0-rc.2
candidate 0.3.0-rc.2
mkdir -p docs
echo "install v0.3.0" > docs/install.md
git add docs/install.md
git commit -q -m "docs: point the install at 0.3.0"
set_version 0.3.0
bump_030="$(git rev-parse HEAD)"
echo "unrelated post-bump commit" > post.txt
git add post.txt
git commit -q -m "fix: unrelated commit after bump"
tip="$(git rev-parse HEAD)"
git push -q origin main
git checkout -q stale
"${script}" --unsigned 0.3.0
[ "$(git rev-parse 'v0.3.0^{commit}')" = "${bump_030}" ] || { echo "FAIL: default commit is not bump commit"; exit 1; }
[ "$(git rev-parse 'v0.3.0^{commit}')" != "${tip}" ] || { echo "FAIL: default commit fell back to tip"; exit 1; }
[ -z "$(git ls-remote --tags origin refs/tags/v0.3.0)" ] || { echo "FAIL: pushed without --push"; exit 1; }

echo "== positive control: a candidate itself needs no candidate"
git checkout -q main
set_version 0.5.0-rc.1
git push -q origin main
git checkout -q stale
"${script}" --unsigned 0.5.0-rc.1
[ "$(git rev-parse 'v0.5.0-rc.1^{commit}')" = "$(git rev-parse origin/main)" ] || { echo "FAIL: candidate tag target"; exit 1; }

echo "all tag-release checks passed"
