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
# fc-04 (F3): a case-only rename of a test file that drops one of its tests.
cd /tmp && rm -rf repo && mkdir repo && cd repo && git init -q && touch AGENTS.md && mkdir tests
printf '#[test]\nfn alpha() {\n    assert_eq!(1 + 1, 2);\n}\n\n#[test]\nfn beta() {\n    assert_eq!(2 + 2, 4);\n}\n' > tests/Suite.rs
git add -A && git commit -qm "chore: base" && git checkout -qb change
git mv tests/Suite.rs tests/suite.rs
printf '#[test]\nfn alpha() {\n    assert_eq!(1 + 1, 2);\n}\n' > tests/suite.rs
git add -A && git commit -qm "refactor: lower-case the suite"
discipline check --base main --format json > r.json 2>/dev/null; echo "discipline exit: $?"; summ r.json suite.rs
