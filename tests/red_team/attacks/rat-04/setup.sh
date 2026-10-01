#!/usr/bin/env bash
. /work/lab-guard.sh || exit 99
set -euo pipefail
git config --global user.name owner
git config --global user.email owner@lab.invalid
git config --global init.defaultBranch main

cd /tmp
rm -rf setup-repo-4
mkdir setup-repo-4 && cd setup-repo-4

curl -s -X DELETE -H "Authorization: token $GITEA_TOKEN" "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test-4" >/dev/null || true
curl -s -X POST -H "Authorization: token $GITEA_TOKEN" -H "Content-Type: application/json" \
  "$DISCIPLINE_FORGE_URL/api/v1/user/repos" \
  -d '{"name":"rat-test-4","default_branch":"main","auto_init":false}' >/dev/null

git init -q
git remote add origin "http://owner:$GITEA_TOKEN@rt-gitea:3000/owner/rat-test-4.git"

touch AGENTS.md
echo "original protected content" > protected.txt
cat << "TOML" > discipline.toml
[gates.ratified-paths]
enabled = true
severity = "error"
protected_paths = ["protected.txt"]
ratifiers = ["owner"]
agent_logins = ["agent"]
refuse_author_ratification = true
TOML

git add AGENTS.md protected.txt discipline.toml
git commit -qm "chore: base commit"
git push -q -u origin main

# Create Issue #1
curl -s -X POST -H "Authorization: token $GITEA_TOKEN" -H "Content-Type: application/json" \
  "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test-4/issues" \
  -d '{"title":"Issue 1 for protected change","body":"Track protected edit"}' >/dev/null

# Add agent as collaborator
curl -s -X PUT -H "Authorization: token $GITEA_TOKEN" -H "Content-Type: application/json" \
  "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test-4/collaborators/agent" \
  -d '{"permission":"write"}' >/dev/null

# Owner comments with an initial innocent comment
comment_resp=$(curl -s -X POST -H "Authorization: token $GITEA_TOKEN" -H "Content-Type: application/json" \
  "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test-4/issues/1/comments" \
  -d '{"body":"Initial innocent comment"}')

cid=$(echo "$comment_resp" | grep -o "\"id\":[0-9]*" | head -1 | cut -d: -f2)

# Sleep 2 seconds so updated_at is guaranteed strictly greater than created_at
sleep 2

# Comment is edited to insert Owner-ratified-paths:
curl -s -X PATCH -H "Authorization: token $GITEA_TOKEN" -H "Content-Type: application/json" \
  "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test-4/issues/comments/$cid" \
  -d '{"body":"Owner-ratified-paths:\n- protected.txt\n"}' >/dev/null

echo "setup 4 completed, comment $cid edited"
