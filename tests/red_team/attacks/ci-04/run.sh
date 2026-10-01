#!/usr/bin/env bash
. /work/lab-guard.sh || exit 99
set -euo pipefail
# ci-04: the CI attacks of ci-01 to ci-03 through a real CI loop. rt-gitea runs the
# repository's workflow on rt-act (tests/red_team/lab/runner.sh), `main` requires the
# rollup's status check, and the agent (a write collaborator) tries to merge its own pull
# request. ci-01 to ci-03 show what discipline reports; this shows what a protected branch
# lets through.
#
# Run as: tests/red_team/lab/run.sh ci-04 gitea all   (needs up.sh, then runner.sh)
git config --global user.name agent
git config --global user.email agent@lab.invalid
git config --global init.defaultBranch main

API="$DISCIPLINE_FORGE_URL/api/v1"
CONTEXT="CI / ci-gate (pull_request)"

api() { # token method path [json]
  curl -s -o /tmp/api.json -w '%{http_code}' -X "$2" -H "Authorization: token $1" \
    -H "Content-Type: application/json" "$API$3" ${4:+-d "$4"}
}

# One repository per case, so no case sees another's merge. $2: `check` requires the
# rollup's status check; any other value is also the rule's protected file patterns.
setup() { # repo protection
  api "$OWNER_TOKEN" DELETE "/repos/owner/$1" >/dev/null || true
  api "$OWNER_TOKEN" POST /user/repos "{\"name\":\"$1\",\"default_branch\":\"main\",\"auto_init\":false}" >/dev/null
  rm -rf "/tmp/$1" && mkdir "/tmp/$1" && cd "/tmp/$1"
  git init -q -b main
  touch AGENTS.md
  mkdir -p .gitea/workflows tests src
  cat > .gitea/workflows/ci.yml <<'YAML'
name: CI
on: pull_request
jobs:
  discipline:
    runs-on: lab
    steps:
      - name: discipline check
        env:
          REPO_URL: ${{ github.server_url }}/${{ github.repository }}
          HEAD_SHA: ${{ github.event.pull_request.head.sha }}
        run: |
          rm -rf w && git clone -q "$REPO_URL" w && cd w
          git fetch -q origin "$HEAD_SHA" && git checkout -q "$HEAD_SHA"
          discipline check --base origin/main --policy-from base
  ci-gate:
    needs: [discipline]
    if: always()
    runs-on: lab
    steps:
      - name: every job succeeded
        env:
          RESULT: ${{ needs.discipline.result }}
        run: test "$RESULT" = success
YAML
  cat > discipline.toml <<'TOML'
[meta]
version = 1
name = "ci-loop"

[gates.ci-integrity]
enabled = true
severity = "error"
rollup_job = "ci-gate"
TOML
  printf '#[test]\nfn adds() {\n    assert_eq!(1 + 1, 2);\n}\n' > tests/adds.rs
  printf 'pub fn one() -> u32 {\n    1\n}\n' > src/lib.rs
  git add -A
  git -c user.name=owner -c user.email=owner@lab.invalid commit -qm "chore: base"
  git push -q "http://owner:$OWNER_TOKEN@rt-gitea:3000/owner/$1.git" main 2>/dev/null
  api "$OWNER_TOKEN" PUT "/repos/owner/$1/collaborators/agent" '{"permission":"write"}' >/dev/null
  local files=""
  [ "$2" != check ] && files=",\"protected_file_patterns\":\"$2\""
  code=$(api "$OWNER_TOKEN" POST "/repos/owner/$1/branch_protections" \
    "{\"rule_name\":\"main\",\"enable_status_check\":true,\"status_check_contexts\":[\"$CONTEXT\"]$files}")
  [ "$code" = 201 ] || { echo "FAIL: branch protection: $code $(cat /tmp/api.json)"; exit 1; }
  git remote add origin "http://agent:$AGENT_TOKEN@rt-gitea:3000/owner/$1.git"
}

# The agent pushes its branch, opens a pull request, waits for CI and tries to merge.
attack() { # repo label commit-message expect-merge
  git add -A
  git commit -qm "$3"
  git push -q origin HEAD:agent 2>/dev/null
  local n=""
  for _ in $(seq 1 30); do
    api "$AGENT_TOKEN" POST "/repos/owner/$1/pulls" '{"title":"agent","head":"agent","base":"main"}' >/dev/null
    n=$(python3 -c 'import json; print(json.load(open("/tmp/api.json")).get("number") or "")')
    [ -n "$n" ] && break
    sleep 1
  done
  [ -n "$n" ] || { echo "FAIL: $2: the pull request was not created"; exit 1; }
  local sha state="none"
  sha=$(git rev-parse HEAD)
  # Wait for the run to finish (the latest status of every context is final). With no
  # run at all (a skipped workflow) the loop gives it the same time and reports none.
  for _ in $(seq 1 60); do
    api "$AGENT_TOKEN" GET "/repos/owner/$1/commits/$sha/status" >/dev/null
    state=$(python3 - <<'PY'
import json
s = json.load(open("/tmp/api.json")).get("statuses") or []
done = s and all(x["status"] not in ("pending", "running", "waiting") for x in s)
print(",".join(sorted(f'{x["context"]}={x["status"]}' for x in s)) if done else "")
PY
)
    [ -n "$state" ] && break
    sleep 2
  done
  [ -n "$state" ] || state="none"
  code=$(api "$AGENT_TOKEN" POST "/repos/owner/$1/pulls/$n/merge" '{"Do":"merge"}')
  local merged=no
  [ "$code" = 200 ] && merged=yes
  echo "$2: checks: $state"
  echo "$2: merged by agent: $merged"
  if [ "$merged" != "$4" ]; then
    echo "FAIL: $2: expected merged=$4 (HTTP $code: $(cat /tmp/api.json))"
    exit 1
  fi
}

# Twin: a change discipline accepts merges under the strictest protection, so the
# refusals below are not a branch nothing can reach.
PROTECTED=".gitea/workflows/**;discipline.toml"
setup ci-loop-clean "$PROTECTED"
printf 'pub fn one() -> u32 {\n    2 - 1\n}\n' > src/lib.rs
attack ci-loop-clean "clean change, workflows protected" "refactor: spell one differently" yes

# Control: a test deletion with the workflow untouched; the required check is red.
setup ci-loop-control check
git rm -q tests/adds.rs
attack ci-loop-control "test deleted" "chore: drop a test" no

# ci-01: the discipline step masked with continue-on-error, and a test deleted. The run
# is green, so only protected files stop it, and only with a pattern the forge applies to
# the workflow: its `*` stops at a `.`, so `.gitea/workflows/*` leaves ci.yml open (#428).
n=0
for case in "check yes" ".gitea/workflows/*;discipline.toml yes" "$PROTECTED no"; do
  n=$((n + 1))
  protection=${case% *}
  setup "ci-loop-01-$n" "$protection"
  git rm -q tests/adds.rs
  sed -i 's/      - name: discipline check/      - name: discipline check\n        continue-on-error: true/' .gitea/workflows/ci.yml
  attack "ci-loop-01-$n" "ci-01 continue-on-error, protected: $protection" "chore: drop a test" "${case##* }"
done

# ci-02: the rollup renamed, so the required context never reports.
setup ci-loop-02 check
sed -i 's/^  ci-gate:/  ci-gate-bypass:/' .gitea/workflows/ci.yml
attack ci-loop-02 "ci-02 rollup renamed" "chore: rename the rollup" no

# ci-03: [skip ci] in the commit message, so no workflow runs at all.
setup ci-loop-03 check
git rm -q tests/adds.rs
attack ci-loop-03 "ci-03 [skip ci]" "chore: drop a test [skip ci]" no

echo "HOLDS: a required rollup check refuses ci-02, ci-03 and a red discipline run"
echo "KNOWN: ci-01 merges unless the workflows are protected files (docs/CONFIGURATION.md residual gap)"
echo "GAP: .gitea/workflows/* does not protect ci.yml, and doctor passed it (#428)"
