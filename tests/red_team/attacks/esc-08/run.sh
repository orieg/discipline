#!/usr/bin/env bash
. /work/lab-guard.sh || exit 99
set -u
git config --global user.name lab
git config --global user.email lab@lab.invalid
git config --global init.defaultBranch main
cd /tmp && rm -rf repo && mkdir repo && cd repo && git init -q
touch AGENTS.md
mkdir -p tests
echo "fn dummy() {}" > tests/old.rs
echo "fn other() {}" > tests/other.rs
git add -A && git commit -qm "chore: base"
git checkout -qb change
# A directive key spelled with LATIN SMALL LETTER LONG S (U+017F), which Unicode case
# folding maps to "s": records whether the case-insensitive parser arms it.
git rm -q tests/old.rs && git commit -qm "feat: delete old test"
printf 'remove\xc5\xbf: tests/old.rs the old suite moved to tests/other.rs\n' > pr_body.md
discipline check --base main --pr-body-file pr_body.md --format json > result.json 2>/dev/null || rc=$?
echo "discipline exit: ${rc:-0}"
python3 - <<"PY"
import json
d = json.load(open("result.json"))
print("failing:", sorted({v["code"] for o in d["outcomes"] for v in o.get("violations", []) if v["severity"] == "error"}))
print("overrides:", sum(len(o.get("overrides", [])) for o in d["outcomes"]))
PY
