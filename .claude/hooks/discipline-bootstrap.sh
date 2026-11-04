#!/bin/bash
# Written by `discipline hook install --agent claude-code`.
# discipline-hook-file: sums=pinned sha256=3afa870af3a49dff6a2b1f994f9be9b37dab33040ac923fd84855c7a0a3bb438
# A Claude Code cloud session starts on a fresh VM without discipline. This installs
# the pinned release, checksum-verified, so the hooks in .claude/settings.json can
# check the change. Locally it does nothing: install discipline yourself.
# Claude Code cloud sessions run only on repositories hosted on GitHub; elsewhere this
# script never runs in the cloud.
# Pinned by `discipline hook install --pin-sums`: the digests below are this release's
# linux-musl assets, each a subject of its SLSA provenance
# (`gh attestation verify <asset> --repo orieg/discipline`).
set -u
[ "${CLAUDE_CODE_REMOTE:-}" = "true" ] || exit 0
command -v discipline >/dev/null 2>&1 && exit 0

version="v0.30.0"
say() { echo "discipline bootstrap: $*" >&2; }
case "$(uname -m)" in
  x86_64|amd64) arch="x86_64" ;;
  aarch64|arm64) arch="aarch64" ;;
  *) say "unsupported architecture $(uname -m); the hooks cannot check this session"; exit 0 ;;
esac
asset="discipline-${arch}-unknown-linux-musl.tar.gz"
base="https://github.com/orieg/discipline/releases/download/${version}"
dir="$(mktemp -d)"
trap 'rm -rf "${dir}"' EXIT
if ! curl -fsSL --retry 3 -o "${dir}/${asset}" "${base}/${asset}"; then
  say "could not download ${version} (network access level?); the hooks cannot check this session"
  exit 0
fi
case "${arch}" in
  x86_64) want="60fd6c7332df56ee9eb8a4dcad8daf5d9eb30845432dd3639fe752c16fb5e74f" ;;
  aarch64) want="9235859a6c982ef9d6dc201d71c907943088206571dd3009b5276d1faff8ee65" ;;
esac
got="$(sha256sum "${dir}/${asset}" | awk '{ print $1 }')"
if [ -z "${want}" ] || [ "${want}" != "${got}" ]; then
  say "checksum mismatch for ${asset}; not installed"
  exit 0
fi
mkdir -p "${dir}/x" "${HOME}/.local/bin"
if tar -xzf "${dir}/${asset}" -C "${dir}/x" \
  && install -m 0755 "${dir}/x/discipline" "${HOME}/.local/bin/discipline"; then
  say "installed $("${HOME}/.local/bin/discipline" --version)"
  # The hooks call `discipline` by name: put its directory on PATH for the session.
  case ":${PATH}:" in
    *":${HOME}/.local/bin:"*) ;;
    *)
      if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
        echo "export PATH=\"${HOME}/.local/bin:\${PATH}\"" >> "${CLAUDE_ENV_FILE}"
      else
        say "${HOME}/.local/bin is not on PATH; the hooks cannot find discipline"
      fi
      ;;
  esac
else
  say "could not unpack ${asset}; the hooks cannot check this session"
fi
exit 0
