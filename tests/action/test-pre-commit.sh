#!/usr/bin/env bash
# Exercises the hooks in .pre-commit-hooks.yaml through the pre-commit
# framework, against a staged change. Requires `pre-commit` and a `discipline`
# binary on PATH.
#
#   test-pre-commit.sh <discipline-repo> <scratch-dir>
set -euo pipefail

hook_repo="$(cd "${1:?path to the discipline repository}" && pwd)"
scratch="${2:?scratch directory}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Ensure ambient CI pull-request context does not leak into isolated local hook tests
unset GITHUB_EVENT_PATH GITEA_EVENT_PATH FORGEJO_EVENT_PATH PR_TITLE PR_BODY

command -v discipline >/dev/null || { echo "discipline is not on PATH" >&2; exit 2; }
command -v pre-commit >/dev/null || { echo "pre-commit is not on PATH" >&2; exit 2; }

run_hook() { # <fixture-dir> -> writes <fixture-dir>.log, returns the hook's exit code
  local dir="$1" rc=0
  (cd "${dir}" && pre-commit try-repo "${hook_repo}" discipline-system --verbose) \
    > "${dir}.log" 2>&1 || rc=$?
  cat "${dir}.log"
  return "${rc}"
}

echo "== positive control: a clean staged change passes the hook"
"${here}/make-fixture.sh" "${scratch}/pc-clean" clean
(cd "${scratch}/pc-clean" && printf '\nPhase 4 follows.\n' >> docs/plan.md && git add -A)
run_hook "${scratch}/pc-clean" || { echo "clean staged change was rejected" >&2; exit 1; }
grep -q 'Status: PASS' "${scratch}/pc-clean.log"

echo "== negative control: a bad staged change is rejected, for the right reasons"
"${here}/make-fixture.sh" "${scratch}/pc-bad" staged
if run_hook "${scratch}/pc-bad"; then
  echo "bad staged change passed the hook" >&2
  exit 1
fi
# The diagnostic, not just the exit code: a missing binary also exits non-zero.
for gate in assertion-reduction vacuous-tests unsafe-safety-comment time-estimates pii; do
  grep -q "\[${gate}\]" "${scratch}/pc-bad.log" || { echo "hook output does not name ${gate}" >&2; exit 1; }
done
grep -q 'Status: FAILED' "${scratch}/pc-bad.log"

echo "== commit-msg positive control: clean commit message passes"
(cd "${scratch}/pc-clean" && printf 'feat: clean commit\n' > commit_msg.txt)
(cd "${scratch}/pc-clean" && pre-commit try-repo "${hook_repo}" discipline-commit-msg-system --hook-stage commit-msg --commit-msg-filename commit_msg.txt --verbose) > "${scratch}/pc-clean-msg.log" 2>&1
cat "${scratch}/pc-clean-msg.log"
grep -q 'Status: PASS' "${scratch}/pc-clean-msg.log"

echo "== commit-msg negative control: time estimate in commit message is rejected"
(cd "${scratch}/pc-clean" && printf 'feat: plan to ship in 3 weeks\n' > bad_commit_msg.txt)
if (cd "${scratch}/pc-clean" && pre-commit try-repo "${hook_repo}" discipline-commit-msg-system --hook-stage commit-msg --commit-msg-filename bad_commit_msg.txt --verbose) > "${scratch}/pc-bad-msg.log" 2>&1; then
  echo "bad commit message passed the hook" >&2
  exit 1
fi
cat "${scratch}/pc-bad-msg.log"
grep -q '\[time-estimates\]' "${scratch}/pc-bad-msg.log"
grep -q 'Status: FAILED' "${scratch}/pc-bad-msg.log"

echo "pre-commit hooks: all controls behaved"
