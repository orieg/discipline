#!/usr/bin/env bash
# record.sh - produce the five forge records of docs/ROADMAP.md (Phase 11 Step 0
# row 2) against the lab that up.sh started.
#
#   bash tests/forge-lab/record.sh
#
#   1. versions: Gitea (`/api/v1/version`), act_runner, the discipline image;
#   2. a clean pull_request run: scheduled, on the pull request head, PASS, no gate fired;
#   3. an inverted canary: a pull request seeding the `bad` change of make-fixture.sh;
#      the job fails and its JSON report names exactly the expected gate ids;
#   4. the clean pull request squash-merged; the push run on `main` notes that the
#      commit arrived through that merged pull request, and Gitea's router log shows
#      the `commits/{sha}/pull` request it was read through;
#   5. `discipline doctor` against the lab's API, with the test user's token.
#
# Every assertion reads the diagnostic (log line, report field), never an exit code
# alone. Logs and reports land in FORGE_LAB_DIR/records; nothing is written to the
# repository.
set -euo pipefail
# shellcheck source=tests/forge-lab/common.sh
. "$(dirname "$0")/common.sh"
root="$(lab_root)"
rec="${LAB_DIR}/records"
mkdir -p "${rec}"
fail() { echo "FAIL: $*" >&2; exit 1; }
json() { python3 -c "import json,sys; d=json.load(sys.stdin); print($1)"; }

# Wait for the run of <event> on <sha>; print "<status> <job_id>" once it completed.
wait_run() {
  local sha="$1" event="$2" out=""
  for _ in $(seq 1 120); do
    out="$(api GET "repos/${LAB_REPO}/actions/runs?event=${event}&head_sha=${sha}" | python3 -c '
import json, sys
runs = json.load(sys.stdin).get("workflow_runs") or []
done = [r for r in runs if r.get("status") == "completed"]
print(" ".join([str(done[0]["conclusion"]), str(done[0]["id"])]) if done else "")')"
    [ -n "${out}" ] && break
    sleep 5
  done
  [ -n "${out}" ] || fail "no completed ${event} run for ${sha}"
  local conclusion="${out% *}" run="${out#* }"
  local job
  job="$(api GET "repos/${LAB_REPO}/actions/runs/${run}/jobs" | json 'd["jobs"][0]["id"]')"
  echo "${conclusion} ${job}"
}

job_log() { api GET "repos/${LAB_REPO}/actions/jobs/$1/logs"; }

# The JSON report the job printed as `lab-report: <base64>`.
report_of() {
  local log="$1" out="$2"
  grep -o 'lab-report: [A-Za-z0-9+/=]*' "${log}" | tail -1 | cut -d' ' -f2 | base64 -d > "${out}"
  [ -s "${out}" ] || fail "no report in ${log}"
}

# Open a pull request from <branch> carrying the `<kind>` change of make-fixture.sh.
open_pr() {
  local kind="$1" branch="$2" title="$3" body="$4"
  local fx="${LAB_DIR}/fx-${kind}" ws="${LAB_DIR}/ws-${kind}"
  rm -rf "${fx}" "${ws}"
  "${root}/tests/action/make-fixture.sh" "${fx}" "${kind}" >/dev/null
  lab_git clone -q "http://127.0.0.1:${LAB_PORT}/${LAB_REPO}.git" "${ws}"
  git -C "${fx}" diff --binary main work | git -C "${ws}" apply --index
  lab_git -C "${ws}" checkout -q -b "${branch}"
  lab_git -C "${ws}" commit -q -m "${title}"
  lab_git -C "${ws}" push -q origin "HEAD:refs/heads/${branch}"
  local payload
  payload="$(python3 -c 'import json,sys; print(json.dumps({"head":sys.argv[1],"base":"main","title":sys.argv[2],"body":sys.argv[3]}))' \
    "${branch}" "${title}" "${body}")"
  api POST "repos/${LAB_REPO}/pulls" -d "${payload}" | json '" ".join([str(d["number"]), d["head"]["sha"], d["base"]["sha"]])'
}

# 1. Versions.
gitea_version="$(api GET version | json 'd["version"]')"
runner_version="$(docker exec "${LAB_RUNNER}" act_runner --version)"
discipline_version="$(docker run --rm "${DISCIPLINE_IMAGE}" discipline --version)"
{
  echo "gitea ${gitea_version} (${GITEA_IMAGE})"
  echo "${runner_version} (${RUNNER_IMAGE})"
  echo "${discipline_version} (${DISCIPLINE_IMAGE})"
} | tee "${rec}/1-versions.txt"

# 2. A clean pull_request run.
read -r clean_pr clean_head clean_base < <(open_pr clean clean-change \
  "test: cover multiplication" "Adds a multiplication test.")
read -r verdict job < <(wait_run "${clean_head}" pull_request)
job_log "${job}" > "${rec}/2-clean.log"
[ "${verdict}" = success ] || fail "clean run concluded ${verdict} (log: ${rec}/2-clean.log)"
grep -q "checked out ${clean_head}; base main is ${clean_base}" "${rec}/2-clean.log" \
  || fail "clean run: checkout is not the pull request head"
[ "${clean_head}" != "${clean_base}" ] || fail "clean head equals base: vacuous"
grep -q "Status: PASS" "${rec}/2-clean.log" || fail "clean run: no PASS verdict"
report_of "${rec}/2-clean.log" "${rec}/2-clean.json"
bash "${root}/tests/action/assert-report.sh" "${rec}/2-clean.json" pass
echo "2. clean pull request #${clean_pr}: ${verdict}, on ${clean_head}, no gate fired"

# 3. The inverted canary.
read -r bad_pr bad_head bad_base < <(open_pr bad canary \
  "test: simplify" "Simplifies the tests.")
read -r verdict job < <(wait_run "${bad_head}" pull_request)
job_log "${job}" > "${rec}/3-canary.log"
[ "${verdict}" = failure ] || fail "canary run concluded ${verdict} (log: ${rec}/3-canary.log)"
grep -q "checked out ${bad_head}; base main is ${bad_base}" "${rec}/3-canary.log" \
  || fail "canary run: checkout is not the pull request head"
report_of "${rec}/3-canary.log" "${rec}/3-canary.json"
bash "${root}/tests/action/assert-report.sh" "${rec}/3-canary.json" fail \
  assertion-reduction vacuous-tests unsafe-safety-comment time-estimates pii
echo "3. canary pull request #${bad_pr}: ${verdict}, gates: $(python3 -c '
import json,sys; r=json.load(open(sys.argv[1]))
print(" ".join(sorted({v["gate"] for o in r["outcomes"] for v in o["violations"]})))' "${rec}/3-canary.json")"

# 4. Merge the clean pull request; the push run on main reads it back.
api POST "repos/${LAB_REPO}/pulls/${clean_pr}/merge" \
  -d '{"Do":"squash","delete_branch_after_merge":false}' >/dev/null
merged_sha="$(api GET "repos/${LAB_REPO}/pulls/${clean_pr}" | json 'd["merge_commit_sha"]')"
[ -n "${merged_sha}" ] && [ "${merged_sha}" != None ] || fail "pull request #${clean_pr} did not merge"
read -r verdict job < <(wait_run "${merged_sha}" push)
job_log "${job}" > "${rec}/4-push.log"
report_of "${rec}/4-push.log" "${rec}/4-push.json"
# The report's directive notes name the commit (abbreviated), the pull request and
# its author.
note="$(python3 -c '
import json, re, sys
report, sha, number, author = json.load(open(sys.argv[1])), *sys.argv[2:]
pattern = re.compile(r"merged-pr-body: commit ([0-9a-f]{7,40}) arrived through merged pull request #(\d+) \(author (\S+)\)")
for note in report.get("directive_notes", []):
    m = pattern.search(note)
    if m and sha.startswith(m.group(1)) and m.group(2) == number and m.group(3) == author:
        print(note)
        sys.exit(0)
sys.exit("directive notes: " + json.dumps(report.get("directive_notes")))
' "${rec}/4-push.json" "${merged_sha}" "${clean_pr}" "${LAB_OWNER}")" \
  || fail "push run: no note that the merged pull request was read"
# The forge's own router log shows the request the note came from.
docker logs "${LAB_GITEA}" 2>&1 \
  | grep -q "GET /api/v1/repos/${LAB_REPO}/commits/${merged_sha}/pull for .*200 OK" \
  || fail "push run: the forge logged no commits/${merged_sha}/pull request"
[ "${verdict}" = success ] || fail "push run concluded ${verdict} (log: ${rec}/4-push.log)"
echo "4. push run on ${merged_sha}: ${verdict}; ${note}"

# 5. doctor against the lab's API, from a clone of main, on the lab network.
dr="${LAB_DIR}/doctor"
rm -rf "${dr}"
lab_git clone -q "http://127.0.0.1:${LAB_PORT}/${LAB_REPO}.git" "${dr}"
GITEA_TOKEN="$(cat "${LAB_DIR}/tester.token")"
export GITEA_TOKEN
set +e
docker run --rm --network "${LAB_NET}" --user "$(id -u):$(id -g)" -v "${dr}:/w:ro" -w /w \
  -e GITEA_TOKEN -e DISCIPLINE_FORGE=gitea -e DISCIPLINE_FORGE_URL="http://${LAB_GITEA}:3000" \
  -e DISCIPLINE_FORGE_REPO="${LAB_REPO}" -e DISCIPLINE_FORGE_ALLOW_HTTP=1 \
  "${DISCIPLINE_IMAGE}" discipline doctor > "${rec}/5-doctor.txt" 2>&1
doctor_rc=$?
set -e
unset GITEA_TOKEN
cat "${rec}/5-doctor.txt"
[ "${doctor_rc}" -ne 2 ] || fail "doctor could not decide a check (exit 2)"
grep -qi "required-check" "${rec}/5-doctor.txt" || fail "doctor did not read the branch rule"
echo "5. doctor exit ${doctor_rc}"
echo "forge records: all five hold; files in ${rec}"
