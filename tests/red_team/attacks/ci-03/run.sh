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

# Probe both [skip ci] and [ci skip] commit message evasions
for pattern in "[skip ci]" "[ci skip]"; do
  git checkout -q -B "pr-skip-ci-$(echo "$pattern" | tr -d '[] ')" main
  git rm -q tests/old.rs
  git commit -qm "feat: drop test $pattern"

  rc=0
  discipline check --base main --format json > result.json 2>/dev/null || rc=$?
  echo "probe '$pattern' exit: ${rc:-0}"

  if [ "${rc:-0}" -ne 1 ]; then
    echo "FAIL: expected exit 1 from deletion-rationale, got ${rc:-0}"
    exit 1
  fi

  PATTERN="$pattern" python3 - <<'PY' || exit $?
import json, os, sys
pattern = os.environ.get("PATTERN", "")
d = json.load(open("result.json"))
if d.get("status") == "could_not_check":
    print("FAIL: check status is could_not_check")
    sys.exit(2)
violations = sorted({
    v.get("code")
    for o in d.get("outcomes", [])
    for v in o.get("violations", [])
    if v.get("severity") == "error"
})
overrides = sum(len(o.get("overrides", [])) for o in d.get("outcomes", []))
if "deletion-rationale/file-deleted-without-rationale" not in violations:
    print(f"FAIL: missing deletion-rationale diagnostic for '{pattern}', got {violations}")
    sys.exit(1)
if overrides != 0:
    print(f"FAIL: override was granted for '{pattern}', overrides={overrides}")
    sys.exit(1)
print("failing:", violations)
print("overrides:", overrides)
PY
done

echo "HOLDS: [skip ci] and [ci skip] markers do not evade discipline sentinels"
exit 0
