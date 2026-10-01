#!/usr/bin/env bash
set -eu

cd /tmp
rm -rf repo
mkdir repo && cd repo
git init -q
git config user.name lab
git config user.email lab@lab.invalid

touch AGENTS.md
mkdir -p tests
echo "fn dummy() {}" > tests/old.rs
git add AGENTS.md tests/old.rs
git commit -qm "chore: base"

git checkout -qb pr-symlink-escape
ln -s /etc/passwd tests/symlink_passwd.rs
git add tests/symlink_passwd.rs
git commit -qm "chore: add symlink outside workspace"

rc=0
discipline check --base main --format json > result.json 2> stderr.txt || rc=$?
echo "discipline exit: ${rc:-0}"

# Security Observable: discipline must never leak target contents (e.g. root:x:0:0 from /etc/passwd)
if grep -q "root:x:0:0" result.json stderr.txt; then
  echo "FAIL: host file content leaked via symlink traversal"
  exit 1
fi
echo "traversal leak: none"

if [ "${rc:-0}" -ne 0 ]; then
  echo "FAIL: unexpected failure exit code: ${rc:-0}"
  exit 1
fi

python3 - <<'PY' || exit $?
import json, sys
d = json.load(open("result.json"))
status = d.get("status", "")
if status != "pass":
    print(f"FAIL: expected status pass, got {status}")
    sys.exit(1)
print("status:", status)
violations = sorted({
    v.get("code")
    for o in d.get("outcomes", [])
    for v in o.get("violations", [])
    if v.get("severity") == "error"
})
print("failing:", violations)
PY

echo "HOLDS: symlink handled safely without traversal leak"
exit 0
