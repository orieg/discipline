#!/usr/bin/env bash
# run-all.sh [attack-id...] - Run attacks in the lab and check each against its expected.txt.
#
# An attack passes when its container exits 0 and every line of its expected.txt appears in
# its output (the scripts assert their own verdicts; expected.txt records the lines a reader
# checks). With no ids, every attack under attacks/ runs. Offline attacks run with
# --network none; rat-*, sec-01 and ci-04 need the lab forges (up.sh), and ci-04 the runner
# (runner.sh). An attack the lab cannot run is reported as such, never as a pass.
set -uo pipefail

DIR="$(cd "$(dirname "$0")/.." && pwd)"
OUT_ROOT="${DISCIPLINE_RT_OUT_ROOT:-$(mktemp -d)}"
RUN="$DIR/lab/run.sh"

ids=("$@")
if [ ${#ids[@]} -eq 0 ]; then
  for d in "$DIR"/attacks/*/; do ids+=("$(basename "$d")"); done
fi

running() { docker inspect -f '{{.State.Running}}' "$1" 2>/dev/null | grep -q true; }

pass=0 fail=0 unrun=0
for id in "${ids[@]}"; do
  out="$OUT_ROOT/$id"
  mkdir -p "$out"
  rc=0
  case "$id" in
    rat-*|sec-01|ci-04)
      if ! running rt-gitea || { [ "$id" = ci-04 ] && ! running rt-act; }; then
        echo "NOT RUN  $id (the lab forge$([ "$id" = ci-04 ] && echo " or runner") is not running)"
        unrun=$((unrun + 1))
        continue
      fi
      ;;
  esac
  case "$id" in
    rat-*)
      RT_SCRIPT=setup.sh DISCIPLINE_RT_OUT="$out/setup" bash "$RUN" "$id" gitea owner >/dev/null 2>&1 || rc=$?
      [ "$rc" -eq 0 ] && { DISCIPLINE_RT_OUT="$out" bash "$RUN" "$id" gitea agent >/dev/null 2>&1 || rc=$?; }
      ;;
    sec-01)
      # sec-01 clones rat-03's repository.
      RT_SCRIPT=setup.sh DISCIPLINE_RT_OUT="$out/setup" bash "$RUN" rat-03 gitea owner >/dev/null 2>&1 || rc=$?
      [ "$rc" -eq 0 ] && { DISCIPLINE_RT_OUT="$out/rat-03" bash "$RUN" rat-03 gitea agent >/dev/null 2>&1 || rc=$?; }
      [ "$rc" -eq 0 ] && { DISCIPLINE_RT_OUT="$out" bash "$RUN" sec-01 gitea agent >/dev/null 2>&1 || rc=$?; }
      ;;
    ci-04) DISCIPLINE_RT_OUT="$out" bash "$RUN" ci-04 gitea all >/dev/null 2>&1 || rc=$? ;;
    *) DISCIPLINE_RT_OUT="$out" bash "$RUN" "$id" none >/dev/null 2>&1 || rc=$? ;;
  esac
  missing=()
  if [ -f "$DIR/attacks/$id/expected.txt" ] && [ -f "$out/actual.txt" ]; then
    while IFS= read -r line; do
      [ -z "$line" ] && continue
      grep -qF -- "$line" "$out/actual.txt" || missing+=("$line")
    done < "$DIR/attacks/$id/expected.txt"
  else
    missing+=("(no expected.txt or no output)")
  fi
  if [ "$rc" -eq 0 ] && [ ${#missing[@]} -eq 0 ]; then
    echo "ok       $id"
    pass=$((pass + 1))
  else
    echo "FAIL     $id (exit $rc; $out/actual.txt)"
    for m in "${missing[@]}"; do echo "         missing: $m"; done
    fail=$((fail + 1))
  fi
done

echo "$pass passed, $fail failed, $unrun not run (output: $OUT_ROOT)"
[ "$fail" -eq 0 ] && [ "$unrun" -eq 0 ]
