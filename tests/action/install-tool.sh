#!/usr/bin/env bash
# Installs a pinned, checksum-verified CI tool into ~/.local/bin.
#   install-tool.sh actionlint|act|scorecard|cargo-cyclonedx
# Linux x86_64 only: this is for the hosted CI runners, not developer machines.
set -euo pipefail

version_args=(--version)
archive_member=""
case "${1:?tool name}" in
  actionlint)
    url="https://github.com/rhysd/actionlint/releases/download/v1.7.12/actionlint_1.7.12_linux_amd64.tar.gz"
    sha="8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8"
    ;;
  act)
    url="https://github.com/nektos/act/releases/download/v0.2.89/act_Linux_x86_64.tar.gz"
    sha="0191d6f1f3b716b5c55820032605d05fc3c1cdbf581ebeff655019e5dd1524c0"
    ;;
  scorecard)
    url="https://github.com/ossf/scorecard/releases/download/v5.5.0/scorecard_5.5.0_linux_amd64.tar.gz"
    sha="83b90a05c1540ef1390db1cd5711e5fd04be9c1d8537fb84d39d02092d6a8dff"
    version_args=(version)
    ;;
  cargo-cyclonedx)
    url="https://github.com/CycloneDX/cyclonedx-rust-cargo/releases/download/cargo-cyclonedx-0.5.9/cargo-cyclonedx-x86_64-unknown-linux-musl.tar.xz"
    sha="9bd3e599314f50810c9d98b8b68a617ff9d3cc20873968d90b29d121f6b226ff"
    archive_member="cargo-cyclonedx-x86_64-unknown-linux-musl/cargo-cyclonedx"
    version_args=(cyclonedx --version)
    ;;
  *)
    echo "unknown tool: $1" >&2
    exit 2
    ;;
esac

tmp="$(mktemp -d)"
curl --fail --silent --show-error --location --retry 3 --output "${tmp}/tool.archive" "${url}"
echo "${sha}  ${tmp}/tool.archive" | sha256sum --check --quiet
mkdir -p "${HOME}/.local/bin"
if [ -n "${archive_member}" ]; then
  tar -xf "${tmp}/tool.archive" -C "${HOME}/.local/bin" --strip-components=1 "${archive_member}"
else
  tar -xzf "${tmp}/tool.archive" -C "${HOME}/.local/bin" "$1"
fi
"${HOME}/.local/bin/$1" "${version_args[@]}"
if [ -n "${GITHUB_PATH:-}" ]; then
  echo "${HOME}/.local/bin" >> "${GITHUB_PATH}"
fi
