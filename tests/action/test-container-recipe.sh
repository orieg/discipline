#!/usr/bin/env bash
# Executes the job-container recipe documented in docs/CONFIGURATION.md under
# nektos/act (the engine inside Gitea's act_runner), so the documentation
# cannot drift from something that runs.
#
#   test-container-recipe.sh <image> [act-platform-image]
#
# The fenced YAML block after the `<!-- snippet: gitea-container-recipe` marker
# is extracted verbatim; the only edit is the image placeholder, filled with
# <image> (the image built in this CI run). A throwaway bare repository stands
# in for the forge: `refs/pull/7/head` is the change, `main` the base. Three
# things are asserted, each on the diagnostic rather than on an exit code:
#   a. the job is scheduled (its steps ran under a registered label);
#   b. the checkout is the pull request head, not the default branch;
#   c. a clean change passes and a change that trips known gates fails,
#      naming those gates.
set -euo pipefail

image="${1:?usage: test-container-recipe.sh <image> [act-platform-image]}"
platform="${2:-gitea/runner-images:ubuntu-latest}"
root="$(cd "$(dirname "$0")/../.." && pwd)"
work="${RUNNER_TEMP:-$(mktemp -d)}/container-recipe"
rm -rf "${work}"
mkdir -p "${work}/wf"

# 1. Extract the documented snippet; only the image placeholder is substituted.
python3 - "${root}/docs/CONFIGURATION.md" "${work}/wf/discipline.yml" "${image}" <<'PY'
import re, sys
doc, out, image = sys.argv[1:]
text = open(doc, encoding="utf-8").read()
marker = "<!-- snippet: gitea-container-recipe"
i = text.index(marker)
m = re.search(r"```yaml\n(.*?)```", text[i:], re.S)
block = m.group(1)
indent = min(len(l) - len(l.lstrip()) for l in block.splitlines() if l.strip())
block = "\n".join(l[indent:] for l in block.splitlines()) + "\n"
placeholder = re.compile(r"^(\s*image:\s*).+$", re.M)
assert len(placeholder.findall(block)) == 1, "expected exactly one image line"
block = placeholder.sub(lambda mm: mm.group(1) + image, block)
open(out, "w", encoding="utf-8").write(block)
print(block)
PY

# 2. A bare "forge" with a pull request ref, from the shared fixture builder.
# act binds the job's working directory into the container at the same path,
# so the bare repository lives under that directory and is fetched over file://.
# The workspace itself starts as an empty repository, as on a fresh runner.
make_origin() {
  local kind="$1"
  local dir="${work}/fx-${kind}"
  "${root}/tests/action/make-fixture.sh" "${dir}" "${kind}" >/dev/null
  local ws="${work}/ws-${kind}"
  mkdir -p "${ws}/.origin/fixture"
  git init -q "${ws}"
  local bare="${ws}/.origin/fixture/${kind}.git"
  git clone -q --bare "${dir}" "${bare}"
  git -C "${bare}" update-ref refs/pull/7/head "$(git -C "${dir}" rev-parse work)"
}
make_origin clean
make_origin bad

# 3. The pull_request event the forge would send.
event_for() {
  local kind="$1"
  local dir="${work}/fx-${kind}"
  python3 - "${work}/event-${kind}.json" "$(git -C "${dir}" rev-parse work)" "$(git -C "${dir}" rev-parse main)" <<'PY'
import json, sys
out, head, base = sys.argv[1:]
json.dump({
  "action": "synchronize",
  "number": 7,
  "pull_request": {
    "number": 7,
    "title": "test: a change",
    "body": "Described here.",
    "user": {"login": "author"},
    "head": {"sha": head, "ref": "work"},
    "base": {"sha": base, "ref": "main"},
  },
  "repository": {"default_branch": "main", "full_name": "fixture/clean"},
}, open(out, "w"))
PY
}
event_for clean
event_for bad

run_act() {
  local kind="$1"
  local log="${work}/act-${kind}.log"
  set +e
  ( cd "${work}/ws-${kind}" && act pull_request \
      --workflows "${work}/wf/discipline.yml" \
      --eventpath "${work}/event-${kind}.json" \
      --platform "ubuntu-latest=${platform}" \
      --pull=false \
      --bind \
      --env "GITHUB_SERVER_URL=file://${work}/ws-${kind}/.origin" \
      --env "GITHUB_REPOSITORY=fixture/${kind}" \
      --actor editor ) >"${log}" 2>&1
  local rc=$?
  set -e
  echo "act(${kind}) exit ${rc}"
  return "${rc}"
}

head_of() { git -C "${work}/fx-$1" rev-parse work; }
main_of() { git -C "${work}/fx-$1" rev-parse main; }

# a + b + c (positive control): the clean change is scheduled, lands on the
# pull request head and passes.
run_act clean || { cat "${work}/act-clean.log"; echo "clean change did not pass" >&2; exit 1; }
log="${work}/act-clean.log"
grep -q "Run discipline" "${log}" || { cat "${log}"; echo "(a) the job did not schedule: no step ran" >&2; exit 1; }
grep -q "checked out $(head_of clean); base main is $(main_of clean)" "${log}" || {
  cat "${log}"; echo "(b) checkout is not the pull request head" >&2; exit 1; }
[ "$(head_of clean)" != "$(main_of clean)" ] || { echo "fixture head equals base; the test is vacuous" >&2; exit 1; }
grep -q "Status: PASS" "${log}" || { cat "${log}"; echo "(c) clean change: no PASS verdict in the output" >&2; exit 1; }
echo "clean change: scheduled, on $(head_of clean), passed"

# c (negative control): the bad change fails, and the named gates are the reason.
if run_act bad; then cat "${work}/act-bad.log"; echo "(c) bad change was accepted" >&2; exit 1; fi
log="${work}/act-bad.log"
grep -q "checked out $(head_of bad); base main is $(main_of bad)" "${log}" || {
  cat "${log}"; echo "(b) bad change: checkout is not the pull request head" >&2; exit 1; }
grep -q "Status: FAILED" "${log}" || { cat "${log}"; echo "(c) bad change: no FAIL verdict in the output" >&2; exit 1; }
for gate in assertion-reduction vacuous-tests unsafe-safety-comment time-estimates pii; do
  grep -q "${gate}" "${log}" || { cat "${log}"; echo "(c) bad change: gate ${gate} not named" >&2; exit 1; }
done
echo "bad change: scheduled, on $(head_of bad), rejected by the expected gates"
echo "container recipe: all three assertions hold"
