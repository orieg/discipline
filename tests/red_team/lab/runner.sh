#!/usr/bin/env bash
# runner.sh - Register an act_runner with rt-gitea for the CI-loop attacks (ci-04).
#
# The runner executes jobs in host mode (label `lab:host`) inside its own container: no
# Docker socket is mounted, so a job can reach nothing the runner container cannot. The
# image is `rust:1.98` (the attack image: git, curl, python3, bash) with the static
# act_runner binary copied in; the discipline build is mounted read-only, as for run.sh.
# Run up.sh first.
set -euo pipefail

ARCH=$(uname -m)
PLATFORM="linux/amd64"
if [ "$ARCH" = "arm64" ] || [ "$ARCH" = "aarch64" ]; then
  PLATFORM="linux/arm64"
fi

docker build -q --platform "$PLATFORM" -t discipline-rt-act - >/dev/null <<'DOCKERFILE'
FROM gitea/act_runner:latest AS runner
FROM rust:1.98
COPY --from=runner /usr/local/bin/act_runner /usr/local/bin/act_runner
DOCKERFILE

# The registration token reaches the container through the environment, not argv.
RUNNER_TOKEN=$(docker exec -u git rt-gitea gitea actions generate-runner-token)
export RUNNER_TOKEN

docker rm -f rt-act >/dev/null 2>&1 || true
docker run -d --name rt-act --platform "$PLATFORM" \
  --network discipline-rt-net --cpus 1 --memory 1200m --pids-limit 512 \
  --cap-drop ALL --security-opt no-new-privileges --user 1000:1000 \
  -e HOME=/tmp -e GIT_CONFIG_NOSYSTEM=1 -e RUNNER_TOKEN \
  -e PATH=/target/debug:/usr/local/bin:/usr/local/cargo/bin:/usr/bin:/bin \
  -v "${RT_TARGET_VOLUME:-discipline-rt-target}":/target:ro -w /tmp \
  discipline-rt-act bash -c 'act_runner register --no-interactive --instance http://rt-gitea:3000 \
    --token "$RUNNER_TOKEN" --name rt-act --labels lab:host >/dev/null && unset RUNNER_TOKEN \
    && exec act_runner daemon' >/dev/null # secrets-argv-ok: one-use registration token for throwaway lab Gitea on internal Docker network; container process listing out of scope

for _ in $(seq 1 30); do
  docker logs rt-act 2>&1 | grep -q "declare successfully" && break
  sleep 1
done
docker logs rt-act 2>&1 | grep -q "declare successfully" || {
  echo "rt-act did not register"
  docker logs rt-act 2>&1 | tail -5
  exit 1
}
echo "rt-act registered with rt-gitea (label lab, host mode)."
