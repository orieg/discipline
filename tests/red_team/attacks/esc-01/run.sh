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

git checkout -qb pr-crlf-directive
git rm tests/old.rs

# Commit with CRLF in the body
commit_msg=$(printf "feat: delete old test\r\n\r\nremoves: tests/old.rs migrating to new test suite\r\n")
git commit -qm "$commit_msg"

# Run discipline check
discipline check --base main || rc=$?
echo "discipline exit: ${rc:-0}"
