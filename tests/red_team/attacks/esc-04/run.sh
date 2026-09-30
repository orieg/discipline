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
echo "fn dummy() {}" > tests/old.rs
git add AGENTS.md tests/old.rs
git commit -qm "chore: base"

git checkout -qb pr-subject-directive
git rm tests/old.rs

# Directive in the first line (subject line) of commit
git commit -m "removes: tests/old.rs replaced by new tests"

discipline check --base main || rc=$?
echo "subject directive exit: ${rc:-0}"
