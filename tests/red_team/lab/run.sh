#!/usr/bin/env bash
# run.sh <attack-id> [gitea|forgejo|none] [user]
# Runs an attack in a throwaway container on the isolated lab network or with --network none.
set -euo pipefail

id=$1
forge=${2:-none}
user=${3:-agent}

DIR="$(cd "$(dirname "$0")/.." && pwd)"
LAB="${DISCIPLINE_RT_LAB:-/tmp/discipline-rt-lab}"
OUT_DIR="${DISCIPLINE_RT_OUT:-$DIR/attacks/$id}"

mkdir -p "$OUT_DIR"

# Platform detection
ARCH=$(uname -m)
PLATFORM="linux/amd64"
if [ "$ARCH" = "arm64" ] || [ "$ARCH" = "aarch64" ]; then
  PLATFORM="linux/arm64"
fi

args=(
  --rm
  --platform "$PLATFORM"
  --cpus "${RT_CPUS:-1}"
  --memory "${RT_MEM:-1200m}"
  --pids-limit 512
  --cap-drop ALL
  --security-opt no-new-privileges
  --user 1000:1000
  -e HOME=/tmp
  -e GIT_CONFIG_NOSYSTEM=1
  -e PATH=/target/debug:/usr/local/cargo/bin:/usr/bin:/bin
  -v discipline-rt-target:/target:ro
  -v "$DIR/attacks":/work
  -w /work
)

if [ "$forge" = "none" ]; then
  args+=(--network none)
else
  args+=(
    --network discipline-rt-net
    -e DISCIPLINE_FORGE="$forge"
    -e DISCIPLINE_FORGE_URL="http://rt-$forge:3000"
    -e DISCIPLINE_FORGE_ALLOW_HTTP=1
    --env-file "$LAB/rt-$forge.$user.env"
  )
fi

docker run "${args[@]}" rust:1.98 bash "/work/$id/${RT_SCRIPT:-run.sh}" > "$OUT_DIR/actual.txt" 2>&1 && rc=0 || rc=$?
echo "container exit: $rc" >> "$OUT_DIR/actual.txt"
exit $rc
