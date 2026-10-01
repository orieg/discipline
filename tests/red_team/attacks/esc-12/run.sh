#!/usr/bin/env bash
. /work/lab-guard.sh || exit 99
set -eu

cd /tmp
rm -rf repo
mkdir repo && cd repo
git init -q
git config user.name lab
git config user.email lab@lab.invalid

touch AGENTS.md
cat <<'EOF' > discipline.toml
[meta]
version = 1
name = "t"

[directives]
max_inline_overrides = 1
EOF
git add AGENTS.md discipline.toml
git commit -qm "chore: base with max_inline_overrides = 1"

git checkout -qb pr-inline-override-budget
cat <<'EOF' > docs.md
Phase 2 (1 week). <!-- discipline:allow(time-estimates) -->
Phase 3 (2 weeks). <!-- discipline:allow(time-estimates) -->
EOF
git add docs.md
git commit -qm "docs: add milestones with inline exemptions"

rc=0
discipline check --base main --policy-from base --format json > result.json 2>/dev/null || rc=$?
echo "discipline exit: ${rc:-0}"

if [ "${rc:-0}" -ne 1 ]; then
  echo "FAIL: expected exit 1 from exceeded max_inline_overrides, got ${rc:-0}"
  exit 1
fi

python3 - <<'PY' || exit $?
import json, sys
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
inline_overrides = sum(
    1
    for o in d.get("outcomes", [])
    for ov in o.get("overrides", [])
    if ov.get("source", {}).get("type") == "Inline" or "Inline" in str(ov.get("source"))
)
policy_failures = d.get("policy_failures", [])
if not any("max_inline_overrides" in f for f in policy_failures):
    print(f"FAIL: max_inline_overrides failure missing in policy_failures: {policy_failures}")
    sys.exit(1)
if inline_overrides != 2:
    print(f"FAIL: expected 2 inline overrides, got {inline_overrides}")
    sys.exit(1)
print("failing:", violations)
print("inline overrides:", inline_overrides)
print("policy failures:", policy_failures)
PY

echo "HOLDS: inline override budget exceeded, change rejected"
exit 0
