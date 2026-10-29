#!/usr/bin/env bash
# scripts/tag-release.sh — create (and optionally push) a release tag on an
# explicit commit of main, after checking it is the commit the release is for.
#
#   scripts/tag-release.sh X.Y.Z[-rc.N] [--commit <ref>] [--remote <name>] [--push]
#
# The commit defaults to the version-bump commit on <remote>/main, never the shell's HEAD.
# Before tagging: the commit is on <remote>/main, its Cargo.toml and Cargo.lock
# carry the version, its CI run passed, and the tag exists neither locally nor on
# the remote. A stable X.Y.Z also needs a release candidate: the newest
# vX.Y.Z-rc.N on the remote, whose Release run passed, on an ancestor of the
# commit, with nothing but release metadata changed since (see
# candidate_checked). After tagging, the tag is read back and must dereference to that commit. A pushed
# release tag cannot be moved or deleted without an administrator ruleset bypass,
# so nothing is pushed unless every check passed and --push was given.
#
# --unsigned creates an annotated, unsigned tag; it exists for the tests only,
# and cannot be combined with --push.
set -euo pipefail

die() { echo "tag-release: $*" >&2; exit 1; }

version=""
ref=""
ref_given=false
remote="origin"
push=false
sign=true
while [ "$#" -gt 0 ]; do
  case "$1" in
    --commit) [ "$#" -ge 2 ] || die "--commit needs a value"; ref="$2"; ref_given=true; shift 2 ;;
    --remote) [ "$#" -ge 2 ] || die "--remote needs a value"; remote="$2"; shift 2 ;;
    --push) push=true; shift ;;
    --unsigned) sign=false; shift ;;
    -*) die "unknown option $1" ;;
    *) [ -z "${version}" ] || die "one version only"; version="${1#v}"; shift ;;
  esac
done
[ -n "${version}" ] || die "usage: tag-release.sh X.Y.Z[-rc.N] [--commit <ref>] [--remote <name>] [--push]"
if "${push}" && ! "${sign}"; then
  die "--unsigned cannot be combined with --push"
fi
# The same shapes release.yml triggers on.
printf '%s\n' "${version}" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-.+)?$' ||
  die "'${version}' is not X.Y.Z or X.Y.Z-<pre>"
tag="v${version}"

git fetch --quiet --no-tags "${remote}" main ||
  die "cannot fetch main from ${remote}"
main="$(git rev-parse --verify --quiet "refs/remotes/${remote}/main^{commit}")" ||
  die "${remote}/main does not resolve after fetch"

# An empty --commit is an error, never a fallback: an empty variable is how a
# tag lands on whatever HEAD happens to be.
if "${ref_given}"; then
  [ -n "${ref}" ] || die "--commit is empty"
else
  ref="$(git log -1 --format=%H --grep="^chore(release): bump version to ${version}" "${main}")"
  if [ -z "${ref}" ]; then
    ref="$(git log -1 --format=%H --grep="bump version to ${version}" "${main}")"
  fi
  [ -n "${ref}" ] || die "cannot find version-bump commit for ${version} on ${remote}/main; pass --commit <ref>"
fi
sha="$(git rev-parse --verify --quiet "${ref}^{commit}")" ||
  die "'${ref}' does not name a commit"

git merge-base --is-ancestor "${sha}" "${main}" ||
  die "${sha} is not on ${remote}/main"

manifest="$(git show "${sha}:Cargo.toml" | sed -n 's/^version = "\(.*\)"/\1/p' | head -n 1)"
[ "${manifest}" = "${version}" ] ||
  die "Cargo.toml at ${sha} is version '${manifest}', not ${version}"
locked="$(git show "${sha}:Cargo.lock" |
  awk '/^name = "discipline"$/ { found = 1; next } found { sub(/^version = "/, ""); sub(/"$/, ""); print; exit }')"
[ "${locked}" = "${version}" ] ||
  die "Cargo.lock at ${sha} has discipline '${locked}', not ${version}"

if ! command -v gh > /dev/null 2>&1; then
  die "cannot check commit CI status: gh is not installed or not on PATH"
fi
ci_out="$(gh run list --commit "${sha}" --workflow "CI" --json status,conclusion --jq 'if .[0] then "\(.[0].status) \(.[0].conclusion)" else empty end' 2>&1)" ||
  die "cannot read CI status for commit ${sha} through gh: ${ci_out}"
[ -n "${ci_out}" ] ||
  die "no CI runs found for commit ${sha}"
[ "${ci_out}" = "completed success" ] ||
  die "CI status for commit ${sha} is '${ci_out}', expected 'completed success'"

# The paths a release bump may change after its candidate; anything else (code,
# tests, workflows, scripts, a dependency) was not exercised by the candidate.
release_metadata() {
  case "$1" in
    Cargo.toml | Cargo.lock | fuzz/Cargo.lock | CITATION.cff | README.md | .pre-commit-hooks.yaml) return 0 ;;
    docs/* | man/* | templates/*) return 0 ;;
    *) return 1 ;;
  esac
}

# A manifest or lockfile at <commit> with discipline's own version blanked, so
# two of them compare equal when only that version differs.
without_own_version() {
  local commit="$1" path="$2"
  case "${path}" in
    Cargo.toml)
      git show "${commit}:${path}" 2> /dev/null |
        awk '!done && /^version = "/ { print "version = \"\""; done = 1; next } { print }' ;;
    *)
      git show "${commit}:${path}" 2> /dev/null |
        awk 'own && /^version = "/ { print "version = \"\""; own = 0; next }
             { own = ($0 == "name = \"discipline\""); print }' ;;
  esac
}

# A stable release is tagged only after a candidate of the same version ran the
# release pipeline on the same code: v0.28.0 failed in `verify` after its tag was
# pushed, and a pushed tag cannot move.
candidate_checked() {
  local listing candidate rc_sha run_out path changed=()
  listing="$(git ls-remote --tags "${remote}")" || die "cannot list tags on ${remote}"
  candidate="$(printf '%s\n' "${listing}" | awk '{ print $2 }' |
    sed -n "s#^refs/tags/\(v${version//./\\.}-rc\.[0-9][0-9]*\)\$#\1#p" | sort -V | tail -n 1)"
  [ -n "${candidate}" ] ||
    die "no release candidate v${version}-rc.N on ${remote}; tag and validate one first (docs/ARCHITECTURE.md, Release candidates)"
  rc_sha="$(printf '%s\n' "${listing}" | awk -v ref="refs/tags/${candidate}^{}" '$2 == ref { print $1 }')"
  [ -n "${rc_sha}" ] ||
    rc_sha="$(printf '%s\n' "${listing}" | awk -v ref="refs/tags/${candidate}" '$2 == ref { print $1 }')"
  git cat-file -e "${rc_sha}^{commit}" 2> /dev/null ||
    die "${candidate} points at ${rc_sha}, which is not a commit of ${remote}/main"
  git merge-base --is-ancestor "${rc_sha}" "${sha}" ||
    die "${candidate} (${rc_sha}) is not an ancestor of ${sha}"

  run_out="$(gh run list --workflow release.yml --branch "${candidate}" --json status,conclusion,headSha \
    --jq 'if .[0] then "\(.[0].status) \(.[0].conclusion) \(.[0].headSha)" else empty end' 2>&1)" ||
    die "cannot read the Release run of ${candidate} through gh: ${run_out}"
  [ -n "${run_out}" ] || die "no Release run found for ${candidate}"
  [ "${run_out}" = "completed success ${rc_sha}" ] ||
    die "Release run of ${candidate} is '${run_out}', expected 'completed success ${rc_sha}'"

  while IFS= read -r path; do
    [ -n "${path}" ] || continue
    if ! release_metadata "${path}"; then
      changed+=("${path}")
    elif [[ "${path}" == Cargo.toml || "${path}" == *Cargo.lock ]] &&
      [ "$(without_own_version "${rc_sha}" "${path}")" != "$(without_own_version "${sha}" "${path}")" ]; then
      changed+=("${path}")
    fi
  done < <(git diff --name-only "${rc_sha}" "${sha}")
  [ "${#changed[@]}" -eq 0 ] ||
    die "${candidate} did not run on this code; changed since: ${changed[*]}. Tag a new candidate"
  echo "candidate ${candidate} (${rc_sha}) passed its Release run"
}

case "${version}" in
  *-*) ;;
  *) candidate_checked ;;
esac

if git rev-parse --verify --quiet "refs/tags/${tag}" > /dev/null; then
  die "${tag} already exists locally ($(git rev-parse "${tag}^{commit}"))"
fi
remote_tag="$(git ls-remote --tags "${remote}" "refs/tags/${tag}")" ||
  die "cannot list tags on ${remote}"
[ -z "${remote_tag}" ] || die "${tag} already exists on ${remote}"

if "${sign}"; then
  git tag -s -m "${tag}" "${tag}" "${sha}"
  git tag -v "${tag}" > /dev/null || die "tag ${tag} signature verification failed"
else
  git tag -a -m "${tag}" "${tag}" "${sha}"
fi
tagged="$(git rev-parse --verify "refs/tags/${tag}^{commit}")"
if [ "${tagged}" != "${sha}" ]; then
  git tag -d "${tag}" > /dev/null
  die "${tag} resolved to ${tagged}, expected ${sha}; local tag removed"
fi

echo "${tag} -> ${sha} ($(git log -1 --format=%s "${sha}"))"
if "${push}"; then
  git tag -v "${tag}" > /dev/null || die "tag ${tag} signature verification failed"
  git push "${remote}" "refs/tags/${tag}"
else
  echo "not pushed; push with: git push ${remote} refs/tags/${tag}"
fi
