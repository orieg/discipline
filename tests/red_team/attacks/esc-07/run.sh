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
# F8: a four-backtick fence holds a three-backtick line; CommonMark keeps the block open
# until a fence of four or more backticks, so the directive after it is still code.
git rm -q tests/old.rs && git commit -qm "feat: delete old test"
cat > pr_body.md <<'BODY'
How to waive a deletion, as documentation:
````md
```
removes: tests/old.rs quoted inside a four-backtick fence
````
No waiver is intended.
BODY
discipline check --base main --pr-body-file pr_body.md --format json > result.json 2>/dev/null || rc=$?
echo "discipline exit: ${rc:-0}"
python3 - <<"PY"
import json
d = json.load(open("result.json"))
print("failing:", sorted({v["code"] for o in d["outcomes"] for v in o.get("violations", []) if v["severity"] == "error"}))
print("overrides:", sum(len(o.get("overrides", [])) for o in d["outcomes"]))
PY
