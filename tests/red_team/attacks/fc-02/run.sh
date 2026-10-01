#!/usr/bin/env bash
set -u
git config --global user.name lab
git config --global user.email lab@lab.invalid
git config --global init.defaultBranch main
# fc-02 (F2): a base ref that does not resolve exits 2, never an empty diff.
cd /tmp && rm -rf repo && mkdir repo && cd repo && git init -q && touch AGENTS.md
mkdir tests && printf '#[test]\nfn a() { assert_eq!(1, 1); }\n' > tests/a.rs
git add -A && git commit -qm "chore: base" && git rm -q tests/a.rs && git commit -qm "feat: drop a"
for base in no-such-ref 0000000000000000000000000000000000000000 'HEAD~5'; do
  discipline check --base "$base" --format json > r.json 2>/dev/null; echo "base $base, exit: $?"
done
