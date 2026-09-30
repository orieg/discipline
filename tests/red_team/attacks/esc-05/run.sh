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

git checkout -qb pr-title-directive
git rm tests/old.rs
git commit -qm "chore: remove test"

touch empty_pr_body.md
discipline check --base main --pr-title "removes: tests/old.rs replaced" --pr-body-file empty_pr_body.md || rc=$?
echo "pr title directive exit: ${rc:-0}"
