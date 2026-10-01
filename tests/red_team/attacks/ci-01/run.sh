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
mkdir -p .github/workflows
cat <<'EOF' > .github/workflows/ci.yml
name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683
      - name: run tests
        run: cargo test
EOF

cat <<'EOF' > discipline.toml
[meta]
version = 1
name = "t"

[gates.ci-integrity]
enabled = true
severity = "error"
EOF

git add AGENTS.md .github discipline.toml
git commit -qm "chore: base with clean CI workflow"

git checkout -qb pr-poison-workflow-step
cat <<'EOF' > .github/workflows/ci.yml
name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683
      - name: run tests
        continue-on-error: true
        run: cargo test
EOF
git add .github/workflows/ci.yml
git commit -qm "feat: add continue-on-error to test step"

rc=0
discipline check --base main --policy-from base --format json > result.json 2>/dev/null || rc=$?
echo "discipline exit: ${rc:-0}"

if [ "${rc:-0}" -ne 1 ]; then
  echo "FAIL: expected exit 1 from step-failure-masked-continue-on-error, got ${rc:-0}"
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
overrides = sum(len(o.get("overrides", [])) for o in d.get("outcomes", []))
if "ci-integrity/step-failure-masked-continue-on-error" not in violations:
    print(f"FAIL: missing step-failure-masked-continue-on-error diagnostic, got {violations}")
    sys.exit(1)
print("failing:", violations)
print("overrides:", overrides)
PY

echo "HOLDS: continue-on-error in workflow verification step detected and blocked"
exit 0
