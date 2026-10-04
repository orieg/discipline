#!/usr/bin/env bash
# scripts/tag-release.sh — create (and optionally push) a release tag on an
# explicit commit of main, after checking it is the commit the release is for.
#
#   scripts/tag-release.sh X.Y.Z[-rc.N] [--commit <ref>] [--remote <name>] [--push]
#
# The commit defaults to <remote>/main as just fetched, never the shell's HEAD.
# Before tagging: the commit is on <remote>/main, its Cargo.toml and Cargo.lock
# carry the version, and the tag exists neither locally nor on the remote. After
# tagging, the tag is read back and must dereference to that commit. A pushed
# release tag cannot be moved or deleted (`release tags` ruleset), so nothing is
# pushed unless every check passed and --push was given.
#
# --unsigned creates an annotated, unsigned tag; it exists for the tests only.
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
  ref="${main}"
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

if git rev-parse --verify --quiet "refs/tags/${tag}" > /dev/null; then
  die "${tag} already exists locally ($(git rev-parse "${tag}^{commit}"))"
fi
remote_tag="$(git ls-remote --tags "${remote}" "refs/tags/${tag}")" ||
  die "cannot list tags on ${remote}"
[ -z "${remote_tag}" ] || die "${tag} already exists on ${remote}"

if "${sign}"; then
  git tag -s -m "${tag}" "${tag}" "${sha}"
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
  git push "${remote}" "refs/tags/${tag}"
else
  echo "not pushed; push with: git push ${remote} refs/tags/${tag}"
fi
