#!/usr/bin/env bash
set -euo pipefail

# Builds the RPM package for discipline
# Usage: package_rpm.sh <version> <rpm_arch> <binary_path> [out_dir]
#   rpm_arch: x86_64 | aarch64
#   binary_path: path to compiled discipline executable

VERSION=${1:?version required}
VERSION="${VERSION#v}"
RPM_ARCH=${2:?architecture required (x86_64|aarch64)}
BIN_PATH=${3:?binary path required}
OUT_DIR=${4:-dist}

if [ ! -f "${BIN_PATH}" ]; then
  echo "error: binary '${BIN_PATH}' does not exist" >&2
  exit 1
fi

case "${RPM_ARCH}" in
  x86_64|aarch64) ;;
  amd64) RPM_ARCH="x86_64" ;;
  arm64) RPM_ARCH="aarch64" ;;
  *) echo "error: unsupported rpm architecture '${RPM_ARCH}'" >&2; exit 1 ;;
esac

TOPDIR="$(mktemp -d)"
trap 'rm -rf "${TOPDIR}"' EXIT

mkdir -p "${TOPDIR}"/{BUILD,RPMS,SOURCES,SPECS,SRPMS,BUILDROOT}

cp "${BIN_PATH}" "${TOPDIR}/SOURCES/discipline"
if [ -f "man/man1/discipline.1" ]; then
  cp "man/man1/discipline.1" "${TOPDIR}/SOURCES/discipline.1"
fi
if [ -f "man/man5/discipline.toml.5" ]; then
  cp "man/man5/discipline.toml.5" "${TOPDIR}/SOURCES/discipline.toml.5"
fi

SPEC_FILE="${TOPDIR}/SPECS/discipline.spec"
cat <<EOF > "${SPEC_FILE}"
%define __strip /bin/true
%define __brp_strip %{nil}
%global debug_package %{nil}

Name:           discipline
Version:        ${VERSION}
Release:        1%{?dist}
Summary:        Universal CI/CD gatekeeper and AI coding agent diff sentinel
License:        MIT OR Apache-2.0
URL:            https://github.com/orieg/discipline

%description
Discipline transforms verification rigors from high-assurance algorithm
repositories into a single, declarative static binary wrapped in a composite
action and standalone CLI.

%install
mkdir -p %{buildroot}/usr/bin
install -m 755 %{_sourcedir}/discipline %{buildroot}/usr/bin/discipline
if [ -f %{_sourcedir}/discipline.1 ]; then
    mkdir -p %{buildroot}%{_mandir}/man1
    install -m 644 %{_sourcedir}/discipline.1 %{buildroot}%{_mandir}/man1/discipline.1
fi
if [ -f %{_sourcedir}/discipline.toml.5 ]; then
    mkdir -p %{buildroot}%{_mandir}/man5
    install -m 644 %{_sourcedir}/discipline.toml.5 %{buildroot}%{_mandir}/man5/discipline.toml.5
fi

%files
/usr/bin/discipline
%{_mandir}/man1/discipline.1*
%{_mandir}/man5/discipline.toml.5*

%changelog
EOF

rpmbuild --define "_topdir ${TOPDIR}" --target "${RPM_ARCH}" -bb "${SPEC_FILE}"

mkdir -p "${OUT_DIR}"
cp "${TOPDIR}/RPMS/${RPM_ARCH}"/*.rpm "${OUT_DIR}/"

echo "Built RPM packages in ${OUT_DIR}"
