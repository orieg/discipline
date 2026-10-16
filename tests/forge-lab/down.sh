#!/usr/bin/env bash
# down.sh - remove everything up.sh created: the runner, the job containers it
# started on the lab network, Gitea (its data lives in the container), the network,
# and FORGE_LAB_DIR (tokens, clones, records) when it carries up.sh's marker.
#
#   bash tests/forge-lab/down.sh            keep the records: FORGE_LAB_KEEP=1
set -euo pipefail
# shellcheck source=tests/forge-lab/common.sh
. "$(dirname "$0")/common.sh"

docker rm -f "${LAB_RUNNER}" >/dev/null 2>&1 || true
# Job containers act_runner left behind on the lab network, if any.
if docker network inspect "${LAB_NET}" >/dev/null 2>&1; then
  for c in $(docker network inspect -f '{{range .Containers}}{{.Name}} {{end}}' "${LAB_NET}"); do
    [ "${c}" = "${LAB_GITEA}" ] || docker rm -f "${c}" >/dev/null 2>&1 || true
  done
fi
docker rm -f "${LAB_GITEA}" >/dev/null 2>&1 || true
docker network rm "${LAB_NET}" >/dev/null 2>&1 || true

if [ "${FORGE_LAB_KEEP:-0}" != 1 ] && [ -f "${LAB_DIR}/.forge-lab" ]; then
  rm -rf "${LAB_DIR}"
fi
echo "forge lab down"
