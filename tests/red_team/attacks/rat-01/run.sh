#!/usr/bin/env bash
set -euo pipefail
git config --global user.name agent
git config --global user.email agent@lab.invalid
git config --global init.defaultBranch main

cd /tmp
rm -rf agent-repo
git clone "http://agent:$GITEA_TOKEN@rt-gitea:3000/owner/rat-test.git" agent-repo
cd agent-repo

git checkout -qb agent-branch
echo "agent modified content" > protected.txt
git add protected.txt
git commit -qm "feat: agent modifies protected path"
git push -q -u origin agent-branch

# Open PR #2 closing Issue #1
pr_resp=$(curl -s -X POST -H "Authorization: token $GITEA_TOKEN" -H "Content-Type: application/json" \
  "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test/pulls" \
  -d '{"title":"Agent PR","body":"Closes #1","head":"agent-branch","base":"main"}')

pr_num=$(echo "$pr_resp" | grep -o "\"number\":[0-9]*" | head -1 | cut -d: -f2)
head_sha=$(git rev-parse HEAD)

# Agent attempts self-ratification by commenting on Issue #1
cat << "COMMENT" > comment.json
{"body":"Owner-ratified-paths:\n- protected.txt\n"}
COMMENT

curl -s -X POST -H "Authorization: token $GITEA_TOKEN" -H "Content-Type: application/json" \
  "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test/issues/1/comments" \
  -d @comment.json >/dev/null

# Prepare GITEA_EVENT_PATH
cat << EVENT > event.json
{
  "pull_request": {
    "number": $pr_num,
    "user": { "login": "agent" },
    "head": { "sha": "$head_sha" }
  },
  "repository": {
    "full_name": "owner/rat-test"
  }
}
EVENT

export GITEA_EVENT_PATH="/tmp/agent-repo/event.json"
export GITEA_ACTIONS="true"
export CI="true"
export GITHUB_ACTIONS=""

discipline check --base main --policy-from base --pr-body-file <(echo "Closes #1") || rc=$?
echo "discipline exit: ${rc:-0}"
