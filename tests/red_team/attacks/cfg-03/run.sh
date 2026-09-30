#!/usr/bin/env bash
set -u
git config --global user.name lab
git config --global user.email lab@lab.invalid
git config --global init.defaultBranch main

cd /tmp
rm -rf repo
mkdir repo && cd repo
git init -q

# Base commit: valid discipline.toml
cat << "TOML" > discipline.toml
[gates.assertion-reduction]
enabled = true
severity = "error"
TOML

touch AGENTS.md
git add discipline.toml AGENTS.md
git commit -qm "chore: base config"

# Create branch for adversary PR
git checkout -qb pr-delete-config

# Adversary deletes discipline.toml entirely
git rm discipline.toml
git commit -qm "feat: delete discipline.toml"

# Run discipline check without policy-from base
discipline check --base main || rc=$?
echo "discipline exit: ${rc:-0}"
