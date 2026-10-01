#!/usr/bin/env bash
. /work/lab-guard.sh || exit 99
set -euo pipefail
git config --global user.name owner
git config --global user.email owner@lab.invalid
git config --global init.defaultBranch main

cd /tmp
rm -rf setup-repo-3
mkdir setup-repo-3 && cd setup-repo-3

curl -s -X DELETE -H "Authorization: token $GITEA_TOKEN" "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test-3" >/dev/null || true
curl -s -X POST -H "Authorization: token $GITEA_TOKEN" -H "Content-Type: application/json" \
  "$DISCIPLINE_FORGE_URL/api/v1/user/repos" \
  -d '{"name":"rat-test-3","default_branch":"main","auto_init":false}' >/dev/null

git init -q
git remote add origin "http://owner:$GITEA_TOKEN@rt-gitea:3000/owner/rat-test-3.git"

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
  "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test-3/issues" \
  -d '{"title":"Issue 1 for protected change","body":"Track protected edit"}' >/dev/null

# Add agent as collaborator
curl -s -X PUT -H "Authorization: token $GITEA_TOKEN" -H "Content-Type: application/json" \
  "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test-3/collaborators/agent" \
  -d '{"permission":"write"}' >/dev/null

# Owner comments with legitimate ratification
cat << "COMMENT" > comment.json
{"body":"Owner-ratified-paths:\n- protected.txt\n"}
COMMENT

curl -s -X POST -H "Authorization: token $GITEA_TOKEN" -H "Content-Type: application/json" \
  "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test-3/issues/1/comments" \
  -d @comment.json >/dev/null
