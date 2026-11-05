#!/bin/bash
# #838 launcher: optionally hold a host-wide bench lock for the whole run, then hand over to
# run_pilot.py or run_confirmatory.py. The run directory is the directory holding this script
# (see docs/benchmarks/REPRODUCING.md for its layout).
#
#   start.sh pilot|confirmatory [runner arguments]
#
# Environment:
#   LOCAL_BASE_URL   OpenAI-compatible endpoint of the model server, ending in /v1
#                    (required for agent cells; not read with --dummies-only)
#   LOCAL_API_KEY    its API key; read from the environment only, never written to disk
#   RECALL_LAB_LOCK  optional flock(1) file that other benchmark jobs on the host also take
#
# The executed launcher was this script with the lab's lock path, endpoint and key lookup
# written in; those are host specifics and stay out of the repository.
set -euo pipefail
cd "$(dirname "$0")"
case "${1:-}" in
  pilot|confirmatory) runner="run_$1.py"; shift ;;
  *) echo "usage: $0 pilot|confirmatory [runner arguments]" >&2; exit 2 ;;
esac
if [ -n "${RECALL_LAB_LOCK:-}" ]; then
  exec 9<>"$RECALL_LAB_LOCK"
  flock -n 9 || { echo "bench lock held: $(cat "$RECALL_LAB_LOCK.owner" 2>/dev/null)" >&2; exit 3; }
  echo "suite=recall838_${runner%.py} pid=$$ start=$(date -u +%FT%TZ)" > "$RECALL_LAB_LOCK.owner"
fi
case " $* " in
  *" --dummies-only "*) ;;
  *) for var in LOCAL_BASE_URL LOCAL_API_KEY; do
       [ -n "${!var:-}" ] || { echo "$var is not set" >&2; exit 4; }
     done ;;
esac
exec venv/bin/python "$runner" "$@"
