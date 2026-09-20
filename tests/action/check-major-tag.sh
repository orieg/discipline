#!/usr/bin/env bash
# tests/action/check-major-tag.sh
#
# Verifies that major floating tags (e.g. v0) dereference to the exact commit
# of the latest non-prerelease semver tag in that major series.
#
# Usage:
#   tests/action/check-major-tag.sh [expected_version] [expected_commit]
#
# When arguments are passed, it checks that the major tag for expected_version
# dereferences to expected_commit.
# When run without arguments, it inspects all tags in the local git repository,
# discovers the highest semver release tag for each major version series,
# and asserts that the corresponding major floating tag dereferences to the
# same commit.
set -euo pipefail

if [ "$#" -ge 2 ]; then
  version="$1"
  expected_commit="$2"
  version_no_v="${version#v}"
  major="v${version_no_v%%.*}"

  actual_commit="$(git rev-parse -q --verify "refs/tags/${major}^{commit}" 2>/dev/null || true)"
  if [ -z "${actual_commit}" ]; then
    echo "::error::Major tag '${major}' does not exist" >&2
    exit 1
  fi
  if [ "${actual_commit}" != "${expected_commit}" ]; then
    echo "::error::Major tag '${major}' dereferences to ${actual_commit}, expected ${expected_commit}" >&2
    exit 1
  fi
  echo "PASS: ${major} dereferences to expected commit ${expected_commit}"
  exit 0
fi

release_tags="$(git tag -l 'v*' | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' || true)"

if [ -z "${release_tags}" ]; then
  echo "No semver release tags found in repository; skipping major tag check."
  exit 0
fi

majors="$(echo "${release_tags}" | sed -E 's/^v([0-9]+)\..*/\1/' | sort -n -u)"

errors=0
for m in ${majors}; do
  major_tag="v${m}"

  latest_release="$(echo "${release_tags}" | grep -E "^v${m}\." | sort -V | tail -n 1)"
  if [ -z "${latest_release}" ]; then
    continue
  fi

  latest_commit="$(git rev-parse -q --verify "refs/tags/${latest_release}^{commit}" 2>/dev/null || true)"
  if [ -z "${latest_commit}" ]; then
    echo "::error::Failed to resolve commit for latest release tag '${latest_release}'" >&2
    errors=$((errors + 1))
    continue
  fi

  major_commit="$(git rev-parse -q --verify "refs/tags/${major_tag}^{commit}" 2>/dev/null || true)"
  if [ -z "${major_commit}" ]; then
    echo "::error::Major tag '${major_tag}' does not exist (latest release is '${latest_release}' at ${latest_commit})" >&2
    errors=$((errors + 1))
    continue
  fi

  if [ "${major_commit}" != "${latest_commit}" ]; then
    echo "::error::Major tag '${major_tag}' dereferences to ${major_commit}, but latest non-prerelease tag '${latest_release}' dereferences to ${latest_commit} (drift detected)" >&2
    errors=$((errors + 1))
  else
    echo "PASS: ${major_tag} correctly dereferences to latest release ${latest_release} (${major_commit})"
  fi
done

if [ "${errors}" -gt 0 ]; then
  exit 1
fi
