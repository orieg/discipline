# Sourced by every attack script before its first command. The scripts rewrite the global
# git configuration and delete directories under /tmp; on a host that changes the owner's
# commit identity and files. Refuse anywhere but a container started by
# tests/red_team/lab/run.sh, which sets DISCIPLINE_RT_IN_CONTAINER=1 and HOME=/tmp.
if [ "${DISCIPLINE_RT_IN_CONTAINER:-}" != 1 ] || [ ! -f /.dockerenv ] || [ "${HOME:-}" != /tmp ]; then
  echo "refusing to run: attack scripts run only inside the lab container, through tests/red_team/lab/run.sh" >&2
  exit 99
fi
