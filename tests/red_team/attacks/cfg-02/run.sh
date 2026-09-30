#!/usr/bin/env bash
set -u
git config --global user.name lab
git config --global user.email lab@lab.invalid
git config --global init.defaultBranch main

cd /tmp
rm -rf repo
mkdir repo && cd repo
git init -q

# Base commit: valid discipline.toml with assertion-reduction enabled
cat << "TOML" > discipline.toml
[gates.config-integrity]
enabled = true
severity = "error"

[gates.assertion-reduction]
enabled = true
severity = "error"
TOML

# Add AGENTS.md so we isolate the gates
touch AGENTS.md
git add discipline.toml AGENTS.md
git commit -qm "chore: base config"

# Create branch for adversary PR
git checkout -qb pr-disable-config-integrity

# Adversary turns off config-integrity ITSELF in head discipline.toml
cat << "TOML" > discipline.toml
[gates.config-integrity]
enabled = false
severity = "warning"

[gates.assertion-reduction]
enabled = false
severity = "warning"
TOML

git add discipline.toml
git commit -qm "feat: disable config integrity"

# Run discipline check WITHOUT --policy-from base (default head)
discipline check --base main || rc=$?
echo "discipline exit: ${rc:-0}"
