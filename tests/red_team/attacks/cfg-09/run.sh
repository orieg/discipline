#!/usr/bin/env bash
# Control: a misspelt key is a configuration error, not a silent default (fail closed).
. /work/lab-guard.sh || exit 99
set -u
git config --global user.name lab
git config --global user.email lab@lab.invalid
git config --global init.defaultBranch main
cd /tmp && rm -rf repo && mkdir repo && cd repo && git init -q
cat << "TOML" > discipline.toml
[gates.assertion-reduction]
enabled = true
TOML
git add discipline.toml && git commit -qm "chore: base config"
git checkout -qb change
cat << "TOML" > discipline.toml
[gates.assertion-reduction]
enable = false
TOML
git add discipline.toml && git commit -qm "chore: tidy config"
discipline check --base main --policy-from base --format json > result.json 2> stderr.txt || rc=$?
echo "discipline exit: ${rc:-0}"
head -c 600 stderr.txt
python3 - <<"PY"
import json
try:
    d = json.load(open("result.json"))
except Exception:
    raise SystemExit(0)
for o in d["outcomes"]:
    if o["gate"] == "config-integrity":
        print(json.dumps({"examined": o.get("examined"), "violations": [v["message"] for v in o.get("violations", [])]}))
print("failing:", sorted({v["code"] for o in d["outcomes"] for v in o.get("violations", []) if v["severity"] == "error"}))
PY
