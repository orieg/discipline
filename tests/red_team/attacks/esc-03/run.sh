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

git checkout -qb pr-no-reason
git rm tests/old.rs
git commit -qm "feat: delete old test"

# Just the filename, NO reason!
echo "removes: tests/old.rs" > pr_body.md
discipline check --base main --pr-body-file pr_body.md
echo "exit: $?"
