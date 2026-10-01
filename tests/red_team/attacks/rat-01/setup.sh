#!/usr/bin/env bash
. /work/lab-guard.sh || exit 99
set -euo pipefail
git config --global user.name owner
git config --global user.email owner@lab.invalid
git config --global init.defaultBranch main

cd /tmp
rm -rf setup-repo
mkdir setup-repo && cd setup-repo

curl -s -X DELETE -H "Authorization: token $GITEA_TOKEN" "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test" >/dev/null || true
curl -s -X POST -H "Authorization: token $GITEA_TOKEN" -H "Content-Type: application/json" \
  "$DISCIPLINE_FORGE_URL/api/v1/user/repos" \
  -d '{"name":"rat-test","default_branch":"main","auto_init":false}' >/dev/null

git init -q
git remote add origin "http://owner:$GITEA_TOKEN@rt-gitea:3000/owner/rat-test.git"

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
  "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test/issues" \
  -d '{"title":"Issue 1 for protected change","body":"Track protected edit"}' >/dev/null

# Add agent and stranger as collaborators with write permission
curl -s -X PUT -H "Authorization: token $GITEA_TOKEN" -H "Content-Type: application/json" \
  "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test/collaborators/agent" \
  -d '{"permission":"write"}' >/dev/null
curl -s -X PUT -H "Authorization: token $GITEA_TOKEN" -H "Content-Type: application/json" \
  "$DISCIPLINE_FORGE_URL/api/v1/repos/owner/rat-test/collaborators/stranger" \
  -d '{"permission":"write"}' >/dev/null

echo "setup completed successfully"
