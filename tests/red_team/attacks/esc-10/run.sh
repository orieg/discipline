#!/usr/bin/env bash
. /work/lab-guard.sh || exit 99
set -eu

cd /tmp
rm -rf repo
mkdir repo && cd repo
git init -q -b main
git config user.name lab
git config user.email lab@lab.invalid

touch AGENTS.md
mkdir -p tests
echo "fn dummy() {}" > tests/old.rs
git add AGENTS.md tests/old.rs
git commit -qm "chore: base"

git checkout -qb pr-punct-rationale
git rm -q tests/old.rs
git commit -qm "feat: delete old test"

# Probe each punctuation / single-char non-rationale
for punct in "." "?" "x"; do
  echo "removes: tests/old.rs $punct" > pr_body.md
  rc=0
  discipline check --base main --pr-body-file pr_body.md --format json > result.json 2>/dev/null || rc=$?
  echo "probe '$punct' exit: ${rc:-0}"
  if [ "${rc:-0}" -eq 0 ]; then
    echo "FAIL: rationale '$punct' was accepted (zero exit)"
    exit 1
  fi
  PUNCT="$punct" python3 - <<'PY' || exit $?
import json, os, sys
punct = os.environ.get("PUNCT", "")
d = json.load(open("result.json"))
if d.get("status") == "could_not_check":
    print("FAIL: check status is could_not_check")
    sys.exit(2)
violations = sorted({
    v.get("code")
    for o in d.get("outcomes", [])
    for v in o.get("violations", [])
    if o.get("gate") == "deletion-rationale" and v.get("severity") == "error"
})
overrides = sum(len(o.get("overrides", [])) for o in d.get("outcomes", []))
if "deletion-rationale/file-deleted-without-rationale" not in violations:
    print(f"FAIL: missing deletion-rationale diagnostic for '{punct}', got {violations}")
    sys.exit(1)
if overrides != 0:
    print(f"FAIL: override was armed for '{punct}', overrides={overrides}")
    sys.exit(1)
print("failing:", violations)
print("overrides:", overrides)
PY
done

echo "HOLDS: all punctuation/single-char rationales rejected with deletion-rationale diagnostic"
exit 0
