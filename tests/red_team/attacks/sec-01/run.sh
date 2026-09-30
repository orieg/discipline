#!/usr/bin/env bash
set -euo pipefail
git config --global user.name agent
git config --global user.email agent@lab.invalid

cd /tmp
git clone -q "http://agent:$GITEA_TOKEN@rt-gitea:3000/owner/rat-test-3.git" repo
cd repo
git checkout -q agent-branch

head_sha=$(git rev-parse HEAD)
cat << EVENT > event.json
{
  "pull_request": {
    "number": 1,
    "user": { "login": "agent" },
    "head": { "sha": "$head_sha" }
  },
  "repository": {
    "full_name": "owner/rat-test-3"
  }
}
EVENT

export GITEA_EVENT_PATH="/tmp/repo/event.json"
export GITEA_ACTIONS="true"
export CI="true"
export GITHUB_ACTIONS=""

# Now enforce DISCIPLINE_NO_NETWORK=1
export DISCIPLINE_NO_NETWORK=1

discipline check --base main --policy-from base --pr-body-file <(echo "Closes #1") || rc=$?
echo "discipline exit: ${rc:-0}"
