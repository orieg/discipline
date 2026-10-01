#!/usr/bin/env bash
. /work/lab-guard.sh || exit 99
set -u
git config --global user.name lab
git config --global user.email lab@lab.invalid
git config --global init.defaultBranch main

cd /tmp
rm -rf repo
mkdir repo && cd repo
git init -q

touch AGENTS.md
mkdir -p tests
cat << "RUST" > tests/sample_test.rs
#[test]
fn test_calculation() {
    let val = 40 + 2;
    assert_eq!(val, 42);
    assert!(val > 0);
}
RUST

git add AGENTS.md tests/sample_test.rs
git commit -qm "chore: base"

# Case A: Adversary weakens assert_eq! to assert! without directive
git checkout -qb pr-weaken-assert
cat << "RUST" > tests/sample_test.rs
#[test]
fn test_calculation() {
    let val = 40 + 2;
    assert!(val > 0);
}
RUST
git add tests/sample_test.rs
git commit -qm "feat: drop assertion"

discipline check --base main || rc_a=$?
echo "weaken assert without directive exit: ${rc_a:-0}"

# Case B: Adversary provides allow-assertion-drop directive with valid reason
cat << "BODY" > pr_body.md
allow-assertion-drop: test_calculation refactoring tests to proptest
BODY

discipline check --base main --pr-body-file pr_body.md || rc_b=$?
echo "weaken assert with directive exit: ${rc_b:-0}"
