#!/usr/bin/env bash
set -euo pipefail

# Builds the Debian package for discipline
# Usage: package_deb.sh <version> <deb_arch> <binary_path> [out_dir]
#   deb_arch: amd64 | arm64
#   binary_path: path to compiled discipline executable

VERSION=${1:?version required}
VERSION="${VERSION#v}"
DEB_ARCH=${2:?architecture required (amd64|arm64)}
BIN_PATH=${3:?binary path required}
OUT_DIR=${4:-dist}

if [ ! -f "${BIN_PATH}" ]; then
  echo "error: binary '${BIN_PATH}' does not exist" >&2
  exit 1
fi

case "${DEB_ARCH}" in
  amd64|arm64) ;;
  x86_64) DEB_ARCH="amd64" ;;
  aarch64) DEB_ARCH="arm64" ;;
  *) echo "error: unsupported deb architecture '${DEB_ARCH}'" >&2; exit 1 ;;
esac

STAGE="$(mktemp -d)"
trap 'rm -rf "${STAGE}"' EXIT

mkdir -p "${STAGE}/usr/bin"
mkdir -p "${STAGE}/DEBIAN"
mkdir -p "${STAGE}/usr/share/doc/discipline"

cp "${BIN_PATH}" "${STAGE}/usr/bin/discipline"
chmod 755 "${STAGE}/usr/bin/discipline"

if [ -f "LICENSE-MIT" ]; then
  cp LICENSE-MIT "${STAGE}/usr/share/doc/discipline/copyright"
fi
if [ -f "README.md" ]; then
  cp README.md "${STAGE}/usr/share/doc/discipline/README.md"
fi
if [ -f "man/man1/discipline.1" ]; then
  mkdir -p "${STAGE}/usr/share/man/man1"
  gzip -9cn "man/man1/discipline.1" > "${STAGE}/usr/share/man/man1/discipline.1.gz"
  chmod 644 "${STAGE}/usr/share/man/man1/discipline.1.gz"
fi
if [ -f "man/man5/discipline.toml.5" ]; then
  mkdir -p "${STAGE}/usr/share/man/man5"
  gzip -9cn "man/man5/discipline.toml.5" > "${STAGE}/usr/share/man/man5/discipline.toml.5.gz"
  chmod 644 "${STAGE}/usr/share/man/man5/discipline.toml.5.gz"
fi
# Shell completions, at the paths bash-completion, zsh (Debian's vendor dir) and fish load.
if [ -f "completions/discipline.bash" ]; then
  install -D -m 644 "completions/discipline.bash" "${STAGE}/usr/share/bash-completion/completions/discipline"
fi
if [ -f "completions/_discipline" ]; then
  install -D -m 644 "completions/_discipline" "${STAGE}/usr/share/zsh/vendor-completions/_discipline"
fi
if [ -f "completions/discipline.fish" ]; then
  install -D -m 644 "completions/discipline.fish" "${STAGE}/usr/share/fish/vendor_completions.d/discipline.fish"
fi

cat <<EOF > "${STAGE}/DEBIAN/control"
Package: discipline
Version: ${VERSION}
Section: devel
Priority: optional
Architecture: ${DEB_ARCH}
Maintainer: Discipline Contributors <https://github.com/orieg/discipline>
Homepage: https://orieg.github.io/discipline/
Description: Universal CI/CD gatekeeper and AI coding agent diff sentinel
 Discipline transforms verification rigors from high-assurance algorithm
 repositories into a single, declarative static binary wrapped in a composite
 action and standalone CLI.
EOF

mkdir -p "${OUT_DIR}"
DEB_NAME="discipline_${VERSION}_${DEB_ARCH}.deb"

if command -v dpkg-deb >/dev/null 2>&1; then
  dpkg-deb --build --root-owner-group "${STAGE}" "${OUT_DIR}/${DEB_NAME}"
else
  # Portable pure-Python fallback for systems without dpkg-deb (e.g. macOS developer hosts)
  python3 -c "
import os, sys, io, tarfile, gzip

stage_dir = sys.argv[1]
out_deb = sys.argv[2]

def reset_tarinfo(ti):
    ti.uid = 0
    ti.gid = 0
    ti.uname = 'root'
    ti.gname = 'root'
    ti.mtime = 0
    return ti

# 1. debian-binary
debian_binary = b'2.0\n'

# 2. control.tar.gz
control_buf = io.BytesIO()
with gzip.GzipFile(fileobj=control_buf, mode='wb', mtime=0) as gz:
    with tarfile.open(fileobj=gz, mode='w') as tar:
        debian_dir = os.path.join(stage_dir, 'DEBIAN')
        for item in sorted(os.listdir(debian_dir)):
            full_p = os.path.join(debian_dir, item)
            tar.add(full_p, arcname=f'./{item}', filter=reset_tarinfo)
control_bytes = control_buf.getvalue()

# 3. data.tar.gz
data_buf = io.BytesIO()
with gzip.GzipFile(fileobj=data_buf, mode='wb', mtime=0) as gz:
    with tarfile.open(fileobj=gz, mode='w') as tar:
        for item in sorted(os.listdir(stage_dir)):
            if item == 'DEBIAN':
                continue
            full_p = os.path.join(stage_dir, item)
            tar.add(full_p, arcname=f'./{item}', filter=reset_tarinfo)
data_bytes = data_buf.getvalue()

# 4. ar archive
with open(out_deb, 'wb') as f:
    f.write(b'!<arch>\n')
    for name, data in [('debian-binary', debian_binary), ('control.tar.gz', control_bytes), ('data.tar.gz', data_bytes)]:
        hdr = f'{name:<16}{0:<12}{0:<6}{0:<6}{\"100644\":<8}{len(data):<10}\x60\n'.encode('ascii')
        f.write(hdr)
        f.write(data)
        if len(data) % 2 == 1:
            f.write(b'\n')
" "${STAGE}" "${OUT_DIR}/${DEB_NAME}"
fi

echo "Built ${OUT_DIR}/${DEB_NAME}"
