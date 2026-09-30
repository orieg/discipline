#!/usr/bin/env bash
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
cat << "RUST" > tests/suite.rs
#[test]
fn test_alpha() {
    assert_eq!(1, 1);
}

#[test]
fn test_beta() {
    assert_eq!(2, 2);
}
RUST

git add AGENTS.md tests/suite.rs
git commit -qm "chore: base"

# Adversary silently deletes test_beta() from surviving tests/suite.rs
git checkout -qb pr-delete-test
cat << "RUST" > tests/suite.rs
#[test]
fn test_alpha() {
    assert_eq!(1, 1);
}
RUST

git add tests/suite.rs
git commit -qm "feat: remove test_beta silently"

discipline check --base main || rc_a=$?
echo "stealth test delete exit: ${rc_a:-0}"

# Now provide removes: test_beta <reason>
cat << "BODY" > pr_body.md
removes: test_beta obsolete test removed
BODY

discipline check --base main --pr-body-file pr_body.md || rc_b=$?
echo "stealth test delete with directive exit: ${rc_b:-0}"
