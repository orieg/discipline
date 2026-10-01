#!/usr/bin/env bash
set -u
git config --global user.name lab
git config --global user.email lab@lab.invalid
git config --global init.defaultBranch main
summ() {  # summ <report.json> <word>: exit, failing codes, notes naming <word>
python3 - "$1" "$2" <<"PY"
import json, sys
d = json.load(open(sys.argv[1])); w = sys.argv[2]
print("failing:", sorted({v["code"] for o in d["outcomes"] for v in o.get("violations", []) if v["severity"] == "error"}))
print("could not check:", [c["reason"] for c in d.get("could_not_check", [])])
print("notes naming it:", sorted({o["gate"] for o in d["outcomes"] for n in o.get("notes", []) if w in n}))
PY
}
# fc-05 (F7): a source file whose first bytes are a binary format's magic number.
# `MZ = 0` is valid Python and JavaScript, and `MZ` is the DOS/PE header; a file read as
# binary is skipped by every gate that reads content, so its weakened test must still be
# reported or named.
cd /tmp && rm -rf repo && mkdir repo && cd repo && git init -q && touch AGENTS.md && mkdir tests
printf 'def test_calc():\n    assert 40 + 2 == 42\n    assert 1 < 2\n' > tests/test_calc.py
printf 'test("calc", () => {\n  expect(40 + 2).toBe(42);\n});\n' > tests/calc.test.js
git add -A && git commit -qm "chore: base"
for magic in none MZ; do
  git checkout -q main && git checkout -qB "change-$magic"
  pre=""; [ "$magic" = MZ ] && pre='MZ = 0'
  printf '%s\ndef test_calc():\n    pass\n' "$pre" > tests/test_calc.py
  printf '%s\ntest("calc", () => {\n});\n' "$pre" > tests/calc.test.js
  git add -A && git commit -qm "test: $magic"
  discipline check --base main --format json > r.json 2>/dev/null; echo "[$magic] discipline exit: $?"; summ r.json tests/
done
