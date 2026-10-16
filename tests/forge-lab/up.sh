#!/usr/bin/env bash
# up.sh - a disposable Gitea and act_runner on a private Docker network, with a
# seeded test repository running the documented job-container recipe.
#
#   bash tests/forge-lab/up.sh       then record.sh, then down.sh
#
# What it creates (all names prefixed `forge-lab` / `discipline-forge-lab`):
#   - the network `discipline-forge-lab`;
#   - Gitea (`forge-lab-gitea`, SQLite, registration off), its HTTP port published on
#     127.0.0.1 only (FORGE_LAB_PORT, default 3300);
#   - an admin `labadmin` and a test user `tester`, with random passwords and tokens
#     generated here and kept in FORGE_LAB_DIR (mode 0700, outside the repository);
#   - `tester/sample`, seeded with the base of tests/action/make-fixture.sh, a
#     `discipline.toml` written by `discipline init` of the pinned release, and two
#     workflows: the job-container recipe extracted verbatim from docs/CONFIGURATION.md
#     (pull_request) and its push counterpart (below);
#   - a branch rule on `main`: the recipe's status check is required and
#     `.gitea/workflows/**` are protected files;
#   - `forge-lab-runner` (act_runner), registered with a one-use token passed through
#     the environment. It starts job containers through the host's Docker socket and
#     attaches them to the lab network, where `github.server_url` resolves.
#
# The forge serves plain HTTP inside the private network, so the push workflow sets
# DISCIPLINE_FORGE_ALLOW_HTTP=1 for its API read; the pull-request run makes no
# forge request. No real credential is used, and nothing points at another forge.
set -euo pipefail
# shellcheck source=tests/forge-lab/common.sh
. "$(dirname "$0")/common.sh"
root="$(lab_root)"

docker info >/dev/null 2>&1 || { echo "the Docker daemon is not running" >&2; exit 2; }
avail_kb="$(df -Pk / | awk 'NR==2 {print $4}')"
[ "${avail_kb}" -ge $((20 * 1024 * 1024)) ] || {
  echo "less than 20 GB free on /; refusing to pull images" >&2; exit 2; }

umask 077
if [ -d "${LAB_DIR}" ] && [ ! -f "${LAB_DIR}/.forge-lab" ]; then
  echo "${LAB_DIR} exists and is not a forge-lab directory; set FORGE_LAB_DIR" >&2
  exit 2
fi
mkdir -p "${LAB_DIR}"
touch "${LAB_DIR}/.forge-lab"

docker rm -f "${LAB_RUNNER}" "${LAB_GITEA}" >/dev/null 2>&1 || true
docker network inspect "${LAB_NET}" >/dev/null 2>&1 || docker network create "${LAB_NET}" >/dev/null

# 1. Gitea.
docker run -d --name "${LAB_GITEA}" --network "${LAB_NET}" \
  -p "127.0.0.1:${LAB_PORT}:3000" \
  -e GITEA__security__INSTALL_LOCK=true \
  -e GITEA__database__DB_TYPE=sqlite3 \
  -e GITEA__server__ROOT_URL="http://${LAB_GITEA}:3000/" \
  -e GITEA__server__HTTP_PORT=3000 \
  -e GITEA__service__DISABLE_REGISTRATION=true \
  -e GITEA__actions__ENABLED=true \
  "${GITEA_IMAGE}" >/dev/null
for _ in $(seq 1 60); do
  curl -sf "${LAB_API}/version" >/dev/null 2>&1 && break
  sleep 2
done
curl -sf "${LAB_API}/version" >/dev/null || { docker logs "${LAB_GITEA}" | tail -20; exit 1; }

# 2. Users and tokens.
for u in labadmin tester; do
  adm=""
  [ "${u}" = labadmin ] && adm="--admin"
  docker exec -u git "${LAB_GITEA}" gitea admin user create ${adm} --username "${u}" \
    --email "${u}@lab.invalid" --password "$(openssl rand -hex 16)" \
    --must-change-password=false >/dev/null
  tok="$(docker exec -u git "${LAB_GITEA}" gitea admin user generate-access-token \
    --username "${u}" --token-name lab --scopes all --raw)"
  printf 'Authorization: token %s\n' "${tok}" > "${LAB_DIR}/${u}.header"
  printf '%s\n' "${tok}" > "${LAB_DIR}/${u}.token"
done

# 3. The repository.
api POST user/repos -d "{\"name\":\"${LAB_REPO_NAME}\",\"default_branch\":\"main\",\"private\":false}" >/dev/null

seed="${LAB_DIR}/seed"
rm -rf "${seed}" "${LAB_DIR}/fx-seed"
"${root}/tests/action/make-fixture.sh" "${LAB_DIR}/fx-seed" clean >/dev/null
lab_git clone -q --branch main "${LAB_DIR}/fx-seed" "${seed}"
git -C "${seed}" remote remove origin

# discipline.toml as the pinned release writes it.
docker run --rm --user "$(id -u):$(id -g)" -v "${seed}:/w" -w /w "${DISCIPLINE_IMAGE}" \
  discipline init --name "${LAB_REPO_NAME}" >/dev/null
[ -s "${seed}/discipline.toml" ] || { echo "discipline init wrote no discipline.toml" >&2; exit 1; }

# The pull_request workflow: the documented recipe, verbatim except for the image
# placeholder and the run line, which also writes the JSON report and prints it as one
# base64 line (`lab-report: ...`) so record.sh can read the gate ids from the job log.
mkdir -p "${seed}/.gitea/workflows"
python3 - "${root}/docs/CONFIGURATION.md" "${seed}/.gitea/workflows/discipline.yml" "${DISCIPLINE_IMAGE}" <<'PY'
import re, sys
doc, out, image = sys.argv[1:]
text = open(doc, encoding="utf-8").read()
i = text.index("<!-- snippet: gitea-container-recipe")
block = re.search(r"```yaml\n(.*?)```", text[i:], re.S).group(1)
indent = min(len(l) - len(l.lstrip()) for l in block.splitlines() if l.strip())
block = "\n".join(l[indent:] for l in block.splitlines()) + "\n"
placeholder = re.compile(r"^(\s*image:\s*).+$", re.M)
assert len(placeholder.findall(block)) == 1, "expected exactly one image line"
block = placeholder.sub(lambda m: m.group(1) + image, block)
run = re.compile(r"^(\s*)run: discipline check --fail-on-warnings\n", re.M)
assert len(run.findall(block)) == 1, "expected exactly one discipline run line"
def report(m):
    pad = m.group(1)
    body = [
        "rc=0",
        "discipline check --fail-on-warnings --json-out report.json || rc=$?",
        "echo \"lab-report: $(base64 < report.json | tr -d '\\n')\"",
        "exit \"${rc}\"",
    ]
    return pad + "run: |\n" + "".join(pad + "  " + l + "\n" for l in body)
block = run.sub(report, block)
open(out, "w", encoding="utf-8").write(block)
PY

# The push workflow: the same image and checkout, for the default branch. The forge
# token is given to discipline so the `merged-pr-body` source can read
# `commits/{sha}/pull`.
cat > "${seed}/.gitea/workflows/discipline-push.yml" <<YAML
name: CI Sentinel (push)

on:
  push:
    branches: [main]

jobs:
  discipline:
    runs-on: ubuntu-latest
    container:
      image: ${DISCIPLINE_IMAGE}
    defaults:
      run:
        shell: sh
    steps:
      - name: Check out the pushed commit
        env:
          SERVER_URL: \${{ github.server_url }}
          REPOSITORY: \${{ github.repository }}
          TOKEN: \${{ github.token }}
          SHA: \${{ github.sha }}
          REF_NAME: \${{ github.ref_name }}
        run: |
          set -eu
          git init -q .
          git remote add origin "\${SERVER_URL}/\${REPOSITORY}.git"
          auth="\$(printf 'x-access-token:%s' "\${TOKEN}" | base64 | tr -d '\n')"
          git -c "http.extraHeader=Authorization: Basic \${auth}" fetch -q --no-tags origin \\
            "+refs/heads/\${REF_NAME}:refs/remotes/origin/\${REF_NAME}"
          git checkout -q "\${SHA}"
          echo "checked out \$(git rev-parse HEAD) on \${REF_NAME}"
      - name: Run discipline
        env:
          GITEA_TOKEN: \${{ github.token }}
          DISCIPLINE_FORGE_ALLOW_HTTP: "1" # the lab forge is plain HTTP on a private network
        run: |
          rc=0
          discipline check --fail-on-warnings --json-out report.json || rc=\$?
          echo "lab-report: \$(base64 < report.json | tr -d '\n')"
          exit "\${rc}"
YAML

# `[skip ci]`: the seed push itself is not one of the records.
git -C "${seed}" add -A
lab_git -C "${seed}" commit -q -m "chore: seed the sample repository [skip ci]"
lab_git -C "${seed}" push -q "http://127.0.0.1:${LAB_PORT}/${LAB_REPO}.git" HEAD:refs/heads/main

# 4. Branch rule on main.
api POST "repos/${LAB_REPO}/branch_protections" -d '{
  "rule_name": "main",
  "enable_status_check": true,
  "status_check_contexts": ["CI Sentinel / discipline (pull_request)"],
  "protected_file_patterns": ".gitea/workflows/**",
  "block_on_outdated_branch": true
}' >/dev/null

# 5. The runner.
cat > "${LAB_DIR}/runner.yaml" <<YAML
log:
  level: info
runner:
  file: /data/.runner
  capacity: 1
  labels:
    - "ubuntu-latest:docker://${DISCIPLINE_IMAGE}"
container:
  network: ${LAB_NET}
  force_pull: false
YAML
chmod 0644 "${LAB_DIR}/runner.yaml"
GITEA_RUNNER_REGISTRATION_TOKEN="$(docker exec -u git "${LAB_GITEA}" gitea actions generate-runner-token)"
export GITEA_RUNNER_REGISTRATION_TOKEN
docker run -d --name "${LAB_RUNNER}" --network "${LAB_NET}" \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v "${LAB_DIR}/runner.yaml:/config.yaml:ro" \
  -e CONFIG_FILE=/config.yaml \
  -e GITEA_INSTANCE_URL="http://${LAB_GITEA}:3000" \
  -e GITEA_RUNNER_REGISTRATION_TOKEN \
  -e GITEA_RUNNER_NAME=forge-lab-runner \
  "${RUNNER_IMAGE}" >/dev/null
unset GITEA_RUNNER_REGISTRATION_TOKEN
for _ in $(seq 1 60); do
  docker logs "${LAB_RUNNER}" 2>&1 | grep -q "declare successfully" && break
  sleep 2
done
docker logs "${LAB_RUNNER}" 2>&1 | grep -q "declare successfully" || {
  docker logs "${LAB_RUNNER}" 2>&1 | tail -20; echo "the runner did not register" >&2; exit 1; }

echo "forge lab up: ${LAB_REPO} on a local Gitea, runner registered; state in ${LAB_DIR}"
