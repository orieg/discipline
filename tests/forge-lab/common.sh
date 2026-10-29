# shellcheck shell=bash disable=SC2034 # the variables are read by the scripts that source this file
# Shared settings for the forge lab (sourced by up.sh, record.sh and down.sh).
#
# Every image is pinned by tag and digest; the digest is what runs. Change a pin
# by reading the new digest with `docker buildx imagetools inspect <image>:<tag>`.

GITEA_IMAGE="gitea/gitea:28.1.0@sha256:f505a0d3a41323b786e05942fc7a4075e07ae7c169fd9485649aa55ee543a21a"
RUNNER_IMAGE="gitea/act_runner:0.6.1@sha256:b5c35d6bdbb9bb25e531230bfc7cc663cb751406cbec90a2a891b85fea54de86"
DISCIPLINE_IMAGE="ghcr.io/orieg/discipline:v0.28.1@sha256:b39944c19202669a33e6fd3018811a343e7ba4fd034e47e9b57e713a1c8892e0"

# Names are prefixed so down.sh removes only what up.sh created.
LAB_NET="discipline-forge-lab"
LAB_GITEA="forge-lab-gitea"
LAB_RUNNER="forge-lab-runner"
LAB_PORT="${FORGE_LAB_PORT:-3300}"
LAB_API="http://127.0.0.1:${LAB_PORT}/api/v1"
LAB_OWNER="tester"
LAB_REPO_NAME="sample"
LAB_REPO="${LAB_OWNER}/${LAB_REPO_NAME}"

# Throwaway state (tokens, clones, logs): outside the repository, mode 0700.
LAB_DIR="${FORGE_LAB_DIR:-${TMPDIR:-/tmp}/discipline-forge-lab}"
LAB_DIR="${LAB_DIR%/}"

lab_root() { cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd; }

# curl against the lab API as the test user. The token is read from a header file,
# never passed on the command line.
api() {
  local method="$1" endpoint="$2"
  shift 2
  curl -sS --fail-with-body -X "${method}" -H @"${LAB_DIR}/tester.header" \
    -H 'Content-Type: application/json' "${LAB_API}/${endpoint}" "$@"
}

# git as the test user against the lab forge: the credential travels in an
# environment-provided header, not in a URL or argv.
lab_git() {
  GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=http.extraHeader \
    GIT_CONFIG_VALUE_0="$(cat "${LAB_DIR}/tester.header")" \
    git -c user.name=tester -c user.email=tester@lab.invalid -c commit.gpgsign=false \
    -c init.defaultBranch=main "$@"
}
