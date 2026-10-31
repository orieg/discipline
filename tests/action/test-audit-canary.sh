#!/usr/bin/env bash
# The cumulative-view canary: a history no single pull request's check can judge.
#
# A strict base records its tests' assertion counts in the baseline; a commit pushed
# without a pull request turns `assertion-reduction` off under a directive; a later
# commit guts a test body. Judged from the loosened trunk, the gutting change passes
# `assertion-reduction` (the gate is off), so this asserts what still sees it:
#
#   * `discipline check` reports `test-floor/test-density-regression` (a warning) on the
#     gutting change, against the counts the base side's baseline records;
#   * `discipline audit` names the loosening (`loosened-without-pull-request`) and the
#     drop (`assertion-density` found) on the merged history;
#   * restoring the setting and the assertions clears both, so the canary is not a
#     detector that always fires.
#
#   test-audit-canary.sh <discipline-binary> <scratch-dir>
set -euo pipefail

bin="$(cd "$(dirname "${1:?path to the discipline binary}")" && pwd)/$(basename "$1")"
scratch="${2:?scratch directory}"
command -v jq >/dev/null || { echo "jq is not on PATH" >&2; exit 2; }

# Ambient CI pull-request context must not reach the scratch repository's checks.
unset GITHUB_EVENT_PATH GITEA_EVENT_PATH FORGEJO_EVENT_PATH GITHUB_EVENT_NAME PR_TITLE PR_BODY GITHUB_BASE_REF GITHUB_HEAD_REF
export DISCIPLINE_NO_NETWORK=1

repo="${scratch}/history"
rm -rf "${repo}"
mkdir -p "${repo}/tests"
cd "${repo}"
git init -q -b main
git config user.email "canary@example.invalid"
git config user.name "canary"
git config commit.gpgsign false

cat > discipline.toml <<'EOF'
[meta]
version = 1
name = "audit-canary"

[gates.assertion-reduction]
enabled = true
EOF
cat > Cargo.toml <<'EOF'
[package]
name = "canary"
version = "0.1.0"
edition = "2021"
EOF
cat > tests/parse.rs <<'EOF'
#[test]
fn parses_a_number() {
    let n: i32 = "42".parse().unwrap();
    assert_eq!(n, 42);
    assert!(n > 0);
    assert_ne!(n, 7);
}
EOF
git add -A
git commit -q -m "chore: strict base"
"${bin}" baseline --write > /dev/null
grep -q '^\[assertions\]' discipline-baseline.toml \
  || { echo "::error::baseline --write recorded no assertion counts"; cat discipline-baseline.toml; exit 1; }
git add -A
git commit -q -m "chore: record the baseline"

# The loosening, pushed straight to the trunk with a directive and no pull request.
sed -i.bak 's/^enabled = true$/enabled = false/' discipline.toml && rm discipline.toml.bak
git commit -q -am "chore: switch off assertion-reduction

allow-gate-weakening: assertion-reduction flaky on generated tests"

# The gutting change, on a branch judged from the loosened trunk.
git checkout -q -b gut
cat > tests/parse.rs <<'EOF'
#[test]
fn parses_a_number() {
    let n: i32 = "42".parse().unwrap();
    assert!(n > 0);
}
EOF
git commit -q -am "test: simplify parse test"

report="${scratch}/gut.json"
rc=0
"${bin}" check --base main --policy-from base --format json > "${report}" || rc=$?
[ "${rc}" -eq 0 ] || { echo "::error::expected the gutting change to pass (warnings only), got exit ${rc}"; jq . "${report}"; exit 1; }
jq -e '[.outcomes[] | select(.gate == "assertion-reduction") | .violations[]] | length == 0' "${report}" > /dev/null \
  || { echo "::error::assertion-reduction reported the gutting change although the trunk turned it off"; exit 1; }
# The diagnostic, not just the exit code.
jq -e '[.outcomes[] | select(.gate == "test-floor") | .violations[] | select(.code == "test-floor/test-density-regression" and .severity == "warning" and .file == "tests/parse.rs")] | length == 1' "${report}" > /dev/null \
  || { echo "::error::test-floor did not report the density drop"; jq '.outcomes[] | select(.gate == "test-floor")' "${report}"; exit 1; }

git checkout -q main
git merge -q --ff-only gut

audit="${scratch}/audit.json"
"${bin}" audit --last 2 --ref main --format json --output "${audit}"
jq -e '[.signals[] | select(.id == "loosened-without-pull-request")] | length == 1' "${audit}" > /dev/null \
  || { echo "::error::audit did not name the loosening pushed without a pull request"; jq '.signals' "${audit}"; exit 1; }
jq -e '[.checks[] | select(.id == "assertion-density" and .state == "found")] | length == 1' "${audit}" > /dev/null \
  || { echo "::error::audit did not find the assertion-density drop"; jq '.checks, .density' "${audit}"; exit 1; }
jq -e '.density.below == [{"path": "tests/parse.rs", "recorded": 4, "current": 2}]' "${audit}" > /dev/null \
  || { echo "::error::audit's density readout is not the drop seeded"; jq '.density' "${audit}"; exit 1; }

# Resolve it: restore the setting and the assertions. The density check comes back clean.
git checkout -q HEAD~2 -- tests/parse.rs discipline.toml
git commit -q -am "test: restore the parse test's assertions and assertion-reduction"
"${bin}" audit --last 1 --ref main --format json --output "${audit}"
jq -e '[.checks[] | select(.id == "assertion-density" and .state == "clean")] | length == 1' "${audit}" > /dev/null \
  || { echo "::error::audit still finds a density drop after the assertions were restored"; jq '.checks, .density' "${audit}"; exit 1; }
rc=0
"${bin}" check --base HEAD~1 --format json > "${report}" || rc=$?
[ "${rc}" -eq 0 ] || { echo "::error::the restoring change did not pass, exit ${rc}"; jq . "${report}"; exit 1; }
jq -e '[.outcomes[].violations[] | select(.code == "test-floor/test-density-regression")] | length == 0' "${report}" > /dev/null \
  || { echo "::error::the density warning outlived the restored assertions"; exit 1; }

echo "audit canary: the loosening and the gutted test were named, and cleared once restored"
