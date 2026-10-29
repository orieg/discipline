#!/bin/bash
# Written by `discipline hook install --agent claude-code`.
# discipline-hook-file: sums=pinned sha256=2db585479ad387dbd95b06f37dd6f9641efc0b082dc843e54920d2dc8e773d2e
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

version="v0.28.1"
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
  x86_64) want="854ef439eece288917a9d23d4cf9e6a0ca1d0d0b2818da81f10168d5fc27c6eb" ;;
  aarch64) want="c53f347709b1be143e5fbf89f48cb7ab5abc01d9282b128eaed37cb73eea96bd" ;;
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
