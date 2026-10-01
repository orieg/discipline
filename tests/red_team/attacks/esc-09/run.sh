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
# directives.max_overrides caps what one change may excuse: two deletion waivers under a
# budget of one must not both apply.
printf '[meta]\nversion = 1\nname = "t"\n[directives]\nmax_overrides = 1\n' > discipline.toml
git add -A && git commit -qm "chore: budget" && git checkout -q main && git merge -q change && git checkout -q -B change2 main
git rm -q tests/old.rs tests/other.rs && git commit -qm "feat: delete two tests"
printf 'removes: tests/old.rs moved to the integration suite\nremoves: tests/other.rs moved to the integration suite\n' > pr_body.md
discipline check --base main --pr-body-file pr_body.md --format json > result.json 2>/dev/null || rc=$?
echo "discipline exit: ${rc:-0}"
python3 - <<"PY"
import json
d = json.load(open("result.json"))
print("failing:", sorted({v["code"] for o in d["outcomes"] for v in o.get("violations", []) if v["severity"] == "error"}))
print("overrides:", sum(len(o.get("overrides", [])) for o in d["outcomes"]))
hits = []
def walk(x):
    if isinstance(x, dict):
        for v in x.values(): walk(v)
    elif isinstance(x, list):
        for v in x: walk(v)
    elif isinstance(x, str) and ("max_overrides" in x or "budget" in x.lower()):
        hits.append(x[:200])
walk(d)
print("budget mentions:", sorted(set(hits))[:3])
PY
