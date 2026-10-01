#!/usr/bin/env bash
set -u
git config --global user.name lab
git config --global user.email lab@lab.invalid
git config --global init.defaultBranch main
# fc-01 (F2): a shallow clone whose base is not fetched, or lacks the merge base, exits 2.
cd /tmp && rm -rf origin repo1 repo2 && mkdir origin && cd origin && git init -q
mkdir tests && printf '#[test]\nfn a() { assert_eq!(1, 1); }\n' > tests/a.rs && touch AGENTS.md
git add -A && git commit -qm "chore: base" && for i in 1 2 3; do echo "$i" > n.txt; git add -A; git commit -qm "chore: $i"; done
git checkout -qb change && git rm -q tests/a.rs && git commit -qm "feat: drop a"
cd /tmp && git clone -q --depth 1 --branch change file:///tmp/origin repo1 && cd repo1
discipline check --base origin/main --format json > r.json 2>err.txt; echo "no base fetched, exit: $?"; head -c 200 err.txt; echo
cd /tmp && git clone -q --depth 1 --no-single-branch --branch change file:///tmp/origin repo2 && cd repo2
discipline check --base origin/main --format json > r.json 2>err.txt; echo "base fetched, no merge base, exit: $?"; head -c 200 err.txt; echo
