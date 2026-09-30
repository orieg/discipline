#!/usr/bin/env bash
# up.sh - Spin up isolated Docker network and mock forges (Gitea and Forgejo)
set -euo pipefail
LAB="${DISCIPLINE_RT_LAB:-/tmp/discipline-rt-lab}"
mkdir -p "$LAB"
umask 077

docker network create --internal discipline-rt-net >/dev/null 2>&1 || true

start() { # name image cli env-prefix
  docker rm -f "$1" >/dev/null 2>&1 || true
  docker run -d --name "$1" --network discipline-rt-net --cpus 1 --memory 1g \
    -e "$4__security__INSTALL_LOCK=true" -e "$4__database__DB_TYPE=sqlite3" \
    -e "$4__server__ROOT_URL=http://$1:3000/" -e "$4__server__HTTP_PORT=3000" \
    -e "$4__service__DISABLE_REGISTRATION=true" ${EXTRA[@]+"${EXTRA[@]}"} "$2" >/dev/null
  for _ in $(seq 1 30); do
    docker exec "$1" wget -qO- http://localhost:3000/api/v1/version >/dev/null 2>&1 && break
    sleep 2
  done
  for u in owner agent stranger; do
    adm=""
    [ "$u" = owner ] && adm="--admin"
    docker exec -u git "$1" "$3" admin user create $adm --username "$u" --email "$u@lab.invalid" \
      --password "$(openssl rand -hex 16)" --must-change-password=false >/dev/null
    printf 'GITEA_TOKEN=%s\n' "$(docker exec -u git "$1" "$3" admin user generate-access-token \
      --username "$u" --token-name lab --scopes all --raw)" > "$LAB/$1.$u.env"
  done
}

EXTRA=(-e GITEA__actions__ENABLED=true -e GITEA__actions__DEFAULT_ACTIONS_URL=self)
start rt-gitea gitea/gitea:1.24 gitea GITEA

EXTRA=()
start rt-forgejo codeberg.org/forgejo/forgejo:12 forgejo FORGEJO

echo "Lab forges initialized. Tokens saved to $LAB (mode 0700)."
