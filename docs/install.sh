#!/usr/bin/env bash
# install.sh — Non-cargo shell installer for Discipline
# Downloads pre-built release binary, verifies SHA256 checksum, and installs it.
#
# Usage:
#   # Recommended: download, inspect, and run
#   curl -fsSL -o install.sh https://orieg.github.io/discipline/install.sh && bash install.sh
#   # Or via one-liner:
#   curl -fsSL https://orieg.github.io/discipline/install.sh | bash
#   ./install.sh [--to <dir>] [--version <version>] [--download-url <url>]
#
set -euo pipefail

fail() {
  echo "error: $*" >&2
  exit 1
}

# Parse options
DEST_DIR="${DEST_DIR:-}"
VERSION="${VERSION:-}"
DOWNLOAD_URL="${DOWNLOAD_URL:-https://github.com/orieg/discipline/releases}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --to)
      DEST_DIR="$2"
      shift 2
      ;;
    --version)
      VERSION="$2"
      shift 2
      ;;
    --download-url)
      DOWNLOAD_URL="$2"
      shift 2
      ;;
    -h|--help)
      echo "Usage: install.sh [--to <dir>] [--version <version>] [--download-url <url>]"
      echo ""
      echo "Options:"
      echo "  --to <dir>            Install directory (default: /usr/local/bin or ~/.local/bin)"
      echo "  --version <v>         Release tag to install (e.g. v0.1.0; default: latest)"
      echo "  --download-url <url>  Base release download URL (for mirrors/testing)"
      exit 0
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
done

# Architecture detection
case "$(uname -m)" in
  x86_64|amd64) arch="x86_64" ;;
  aarch64|arm64) arch="aarch64" ;;
  *) fail "unsupported architecture: $(uname -m)" ;;
esac

# OS detection
case "$(uname -s)" in
  Linux) triple="${arch}-unknown-linux-musl" ;;
  Darwin) triple="${arch}-apple-darwin" ;;
  *) fail "unsupported OS: $(uname -s)" ;;
esac

# Determine destination directory
if [ -z "${DEST_DIR}" ]; then
  if [ -w "/usr/local/bin" ]; then
    DEST_DIR="/usr/local/bin"
  else
    DEST_DIR="${HOME}/.local/bin"
  fi
fi

# Validate write permissions on destination directory or its parent
if [ -d "${DEST_DIR}" ]; then
  if [ ! -w "${DEST_DIR}" ]; then
    fail "destination directory '${DEST_DIR}' is not writable (try running with sudo or specify a user directory with --to)"
  fi
else
  parent_dir="$(dirname "${DEST_DIR}")"
  while [ ! -d "${parent_dir}" ] && [ "${parent_dir}" != "/" ] && [ "${parent_dir}" != "." ]; do
    parent_dir="$(dirname "${parent_dir}")"
  done
  if [ ! -w "${parent_dir}" ]; then
    fail "cannot create destination directory '${DEST_DIR}': parent '${parent_dir}' is not writable"
  fi
fi
mkdir -p "${DEST_DIR}"

# Download URLs
if [ -n "${VERSION}" ]; then
  base="${DOWNLOAD_URL}/download/${VERSION}"
else
  base="${DOWNLOAD_URL}/latest/download"
fi

archive="discipline-${triple}.tar.gz"
tmp_dir="$(mktemp -d 2>/dev/null || mktemp -d -t 'discipline-install')"
trap 'rm -rf "${tmp_dir}"' EXIT

fetch() {
  local url="$1" out="$2"
  if command -v curl >/dev/null 2>&1; then
    curl --fail --silent --show-error --location --retry 3 --output "${out}" "${url}"
  elif command -v wget >/dev/null 2>&1; then
    wget --quiet --tries=3 --output-document="${out}" "${url}"
  else
    fail "neither curl nor wget is available"
  fi
}

echo "Downloading discipline (${triple})..."
fetch "${base}/${archive}" "${tmp_dir}/${archive}" || fail "could not download ${base}/${archive}"
fetch "${base}/SHA256SUMS" "${tmp_dir}/SHA256SUMS" || fail "could not download ${base}/SHA256SUMS"

# SHA256 checksum verification
echo "Verifying SHA256 checksum..."
expected="$(awk -v f="${archive}" '$2 == f || $2 == "*" f { print $1 }' "${tmp_dir}/SHA256SUMS")"
[ -n "${expected}" ] || fail "${archive} is not listed in SHA256SUMS"

if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "${tmp_dir}/${archive}" | awk '{ print $1 }')"
elif command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "${tmp_dir}/${archive}" | awk '{ print $1 }')"
else
  fail "neither sha256sum nor shasum is available for checksum verification"
fi

if [ "${expected}" != "${actual}" ]; then
  fail "checksum mismatch for ${archive}: expected ${expected}, got ${actual}"
fi

# Extract and install
tar -xzf "${tmp_dir}/${archive}" -C "${tmp_dir}"
[ -x "${tmp_dir}/discipline" ] || fail "archive did not contain an executable 'discipline' binary"

cp "${tmp_dir}/discipline" "${DEST_DIR}/discipline"
chmod +x "${DEST_DIR}/discipline"

echo "Successfully installed discipline to ${DEST_DIR}/discipline"
"${DEST_DIR}/discipline" --version

case ":${PATH}:" in
  *":${DEST_DIR}:"*) ;;
  *)
    echo "Note: ${DEST_DIR} is not currently in your PATH."
    echo "Add it with: export PATH=\"${DEST_DIR}:\$PATH\""
    ;;
esac
