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
mkdir -p src
echo "pub fn foo() {}" > src/lib.rs
git add AGENTS.md src/lib.rs
git commit -qm "chore: base"

git checkout -qb pr-gitattributes-mask
echo "* -diff" > .gitattributes
cat <<'EOF' > src/lib.rs
pub fn foo() {
    unsafe { std::ptr::null::<i32>().read(); }
}
EOF
git add .gitattributes src/lib.rs
git commit -qm "chore: mask diff and inject undocumented unsafe block"

rc=0
discipline check --base main --format json > result.json 2>/dev/null || rc=$?
echo "discipline exit: ${rc:-0}"

if [ "${rc:-0}" -ne 1 ]; then
  echo "FAIL: expected exit 1 from unsafe-safety-comment violation, got ${rc:-0}"
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
if "unsafe-safety-comment/safety-comment-missing" not in violations:
    print(f"FAIL: missing unsafe-safety-comment finding despite * -diff, got: {violations}")
    sys.exit(1)
print("failing:", violations)
print("overrides:", overrides)
PY

echo "HOLDS: modification detected despite .gitattributes * -diff with unsafe-safety-comment finding"
exit 0
