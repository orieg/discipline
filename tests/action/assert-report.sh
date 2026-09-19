#!/usr/bin/env bash
# Asserts on *why* a discipline run ended the way it did. An inverted canary
# that only checks for a non-zero exit also "passes" when the binary is
# missing or crashes, so the expected gates are matched by name.
#
#   assert-report.sh <report.json> pass
#   assert-report.sh <report.json> fail <gate> [<gate> ...]
set -euo pipefail

report="${1:?report path}"
expect="${2:?pass|fail}"
shift 2

[ -s "${report}" ] || { echo "report ${report} is missing or empty" >&2; exit 1; }

fired() {
  python3 - "$report" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
outcomes = report["outcomes"]
if not outcomes:
    sys.exit("report lists no gates: nothing was checked")
if not any(o["enabled"] and o["examined"] > 0 for o in outcomes):
    sys.exit("every gate examined 0 items: nothing was checked")
print("\n".join(sorted({v["gate"] for o in outcomes for v in o["violations"]})))
PY
}

actual="$(fired)"
case "${expect}" in
  pass)
    [ -z "${actual}" ] || { echo "expected a clean report, gates fired: ${actual}" >&2; exit 1; }
    ;;
  fail)
    wanted="$(printf '%s\n' "$@" | sort -u)"
    [ "${actual}" = "${wanted}" ] || {
      printf 'gate set mismatch\n--- expected\n%s\n--- actual\n%s\n' "${wanted}" "${actual}" >&2
      exit 1
    }
    ;;
  *) echo "expected pass|fail, got ${expect}" >&2; exit 2 ;;
esac

if [ "$#" -gt 0 ]; then
  echo "report matches expectation: ${expect} ${*}"
else
  echo "report matches expectation: ${expect}"
fi
