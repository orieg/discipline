#!/bin/bash
# Written by `discipline hook install --agent claude-code`.
# discipline-hook-file: sums=pinned sha256=99fa9dcef09bf0ebaf646f6e936af746d99be8e59db91199dab934359ea11cb1
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

version="v0.29.0"
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
  x86_64) want="ec84e62c4d898d5a6e1b31c723d6f8acc3d39f7702c7939d9794b11bd501cea9" ;;
  aarch64) want="52e0168078194ee5170b5ac38ae69bf589dd90042447682dd2b5dcef77573d22" ;;
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
