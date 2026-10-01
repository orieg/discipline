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

git checkout -qb pr-homoglyph-rationale
git rm -q tests/old.rs
git commit -qm "feat: delete old test"

check_probe() {
  local label="$1"
  local rc=0
  discipline check --base main --pr-body-file pr_body.md --format json > result.json 2>/dev/null || rc=$?
  echo "probe '$label' exit: ${rc:-0}"
  if [ "${rc:-0}" -eq 0 ]; then
    echo "FAIL: probe '$label' was accepted (zero exit)"
    exit 1
  fi
  LABEL="$label" python3 - <<'PY' || exit $?
import json, os, sys
label = os.environ.get("LABEL", "")
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
    print(f"FAIL: missing deletion-rationale diagnostic for '{label}', got {violations}")
    sys.exit(1)
if overrides != 0:
    print(f"FAIL: override armed for '{label}', overrides={overrides}")
    sys.exit(1)
print("failing:", violations)
print("overrides:", overrides)
PY
}

# Probe 1: Zero-width space in placeholder (t+ZWSP+o+ZWSP+d+ZWSP+o)
printf "removes: tests/old.rs t\xE2\x80\x8Bo\xE2\x80\x8Bd\xE2\x80\x8Bo\n" > pr_body.md
check_probe "zero-width space in placeholder"

# Probe 2: Cyrillic homoglyph (tоdо with \xD0\xBE)
printf "removes: tests/old.rs t\xD0\xBEd\xD0\xBE\n" > pr_body.md
check_probe "Cyrillic homoglyph"

# Probe 3: Fullwidth Latin placeholder (ｔｏｄｏ)
printf "removes: tests/old.rs \xEF\xBD\x94\xEF\xBD\x8F\xEF\xBD\x84\xEF\xBD\x8F\n" > pr_body.md
check_probe "fullwidth Latin"

# Probe 4: Zero-width space separator between placeholders (todo+ZWSP+fixme)
printf "removes: tests/old.rs todo\xE2\x80\x8Bfixme\n" > pr_body.md
check_probe "zero-width separator between placeholders"

# Probe 5: Mathematical bold todo (𝐭𝐨𝐝𝐨)
printf "removes: tests/old.rs \xF0\x9D\x90\xAD\xF0\x9D\x90\xA8\xF0\x9D\x90\x9D\xF0\x9D\x90\xA8\n" > pr_body.md
check_probe "mathematical bold"

# Probe 6: Combining acute mark (to\u0301do)
printf "removes: tests/old.rs to\xCC\x81do\n" > pr_body.md
check_probe "combining acute mark"

# Probe 7: Leetspeak digits (t0d0)
printf "removes: tests/old.rs t0d0\n" > pr_body.md
check_probe "leetspeak digits"

echo "HOLDS: all confusable/invisible placeholder evasions rejected with deletion-rationale diagnostic"
exit 0
