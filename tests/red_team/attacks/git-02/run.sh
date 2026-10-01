#!/usr/bin/env bash
. /work/lab-guard.sh || exit 99
set -eu
git config --global user.name lab
git config --global user.email lab@lab.invalid
git config --global init.defaultBranch main
git config --global protocol.file.allow always

# The adversary deletes a test suite and points a submodule at a repository holding
# "the same tests". Discipline cannot see into the submodule; the deletion must still
# need a `removes:` rationale, and the gitlink must not break the run.
cd /tmp
rm -rf repo suite
mkdir suite && cd suite
git init -q
mkdir -p tests
printf '#[test]\nfn alpha() {\n    assert_eq!(1 + 1, 2);\n}\n' > tests/suite.rs
git add tests/suite.rs
git commit -qm "chore: suite"

cd /tmp
mkdir repo && cd repo
git init -q
touch AGENTS.md
mkdir -p tests/suite
printf '#[test]\nfn alpha() {\n    assert_eq!(1 + 1, 2);\n}\n\n#[test]\nfn beta() {\n    assert_eq!(2 * 2, 4);\n}\n' > tests/suite/suite.rs
git add AGENTS.md tests/suite/suite.rs
git commit -qm "chore: base"

probe() { # label, PR body, expected exit
  rc=0
  discipline check --base main --format json --pr-body-file <(printf '%s\n' "$2") > result.json 2>stderr.txt || rc=$?
  echo "$1 exit: $rc"
  if [ "$rc" -ne "$3" ]; then
    echo "FAIL: $1: expected exit $3"
    exit 1
  fi
  python3 - <<'PY'
import json
d = json.load(open("result.json"))
codes = sorted({v["code"] for o in d["outcomes"] for v in o["violations"] if v["severity"] == "error"})
print("failing:", codes)
print("overrides:", sum(len(o["overrides"]) for o in d["outcomes"]))
PY
}

# 1. The test directory becomes a submodule at the same path.
git checkout -qb pr-submodule-same-path
git rm -q -r tests/suite
git submodule add -q /tmp/suite tests/suite
git commit -qm "refactor: tests live in the suite submodule"
probe "same path" "" 1

# 2. The same change, justified with a `removes:` rationale.
probe "same path, removes:" "removes: tests/suite/suite.rs tests moved to the shared suite repository" 1

# 3. A gitlink with no `.gitmodules` entry (a bare index entry) at a test path.
git checkout -q main 2>/dev/null
git checkout -qb pr-bare-gitlink
git rm -q tests/suite/suite.rs
sha=$(git -C /tmp/suite rev-parse HEAD)
git update-index --add --cacheinfo "160000,$sha,tests/vendored"
git commit -qm "refactor: vendor the suite"
probe "bare gitlink" "" 1

# 4. Tests that already live in a submodule; the change moves its pointer to a commit
#    that deletes them. Their content is outside the repository: the pointer move is
#    named in the notes, not judged (KNOWN, docs/ARCHITECTURE.md residual risks).
git checkout -q -f main
rm -rf tests/suite tests/vendored .git/modules
git checkout -qb base-with-submodule
git rm -q -r tests/suite
rm -rf tests/suite
git submodule add -q /tmp/suite tests/suite
git commit -qm "chore: tests in a submodule"
git branch -q -f main HEAD
git -C /tmp/suite rm -q tests/suite.rs
git -C /tmp/suite commit -qm "chore: drop the suite"
git checkout -qb pr-bump-submodule
git -C tests/suite pull -q origin main
git add tests/suite
git commit -qm "chore: bump the suite"
probe "pointer bump" "" 0
python3 - <<'PY'
import json
d = json.load(open("result.json"))
notes = [n for o in d["outcomes"] for n in o["notes"] if "tests/suite" in n]
print("submodule named in notes:", bool(notes))
if not notes:
    raise SystemExit("FAIL: the submodule pointer move is not named")
PY

echo "HOLDS: tests replaced by a submodule or a bare gitlink are reported as deleted"
echo "KNOWN: a pointer move inside an existing submodule is named, not judged"
