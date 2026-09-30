#!/usr/bin/env bash
set -u
git config --global user.name lab
git config --global user.email lab@lab.invalid
git config --global init.defaultBranch main

cd /tmp
rm -rf repo
mkdir repo && cd repo
git init -q

# Base commit: valid discipline.toml with config-integrity and assertion-reduction enabled
cat << "TOML" > discipline.toml
[gates.config-integrity]
enabled = true
severity = "error"

[gates.assertion-reduction]
enabled = true
severity = "error"
TOML

git add discipline.toml
git commit -qm "chore: base config"

# Create branch for adversary PR
git checkout -qb pr-weaken-config

# Adversary tries to disable gates in discipline.toml
cat << "TOML" > discipline.toml
[gates.config-integrity]
enabled = false
severity = "warning"

[gates.assertion-reduction]
enabled = false
severity = "warning"
TOML

git add discipline.toml
git commit -qm "feat: disable gates in discipline.toml"

# Run discipline check with --policy-from base
discipline check --base main --policy-from base --format json > result.json 2> stderr.txt || rc=$?
echo "discipline exit: ${rc:-0}"
grep -o "\"code\":\"[^\"]*\"" result.json || true
cat stderr.txt
cat result.json
