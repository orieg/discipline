#!/bin/sh
set -e

# Export DISCIPLINE_TRUST_WORKSPACE=1 when not explicitly set and running inside the container
# workspace mount (/workspace) or an ephemeral CI job environment.
if [ -z "${DISCIPLINE_TRUST_WORKSPACE+x}" ]; then
    case "$(pwd)" in
        /workspace|/workspace/*)
            export DISCIPLINE_TRUST_WORKSPACE=1
            ;;
        *)
            if [ -n "${CI:-}" ] || [ -n "${GITHUB_ACTIONS:-}" ] || [ -n "${GITLAB_CI:-}" ] || [ -n "${GITEA_ACTIONS:-}" ] || [ -n "${FORGEJO_ACTIONS:-}" ]; then
                export DISCIPLINE_TRUST_WORKSPACE=1
            fi
            ;;
    esac
fi

# In trusted container/CI environments outside /workspace (which is already registered in
# /etc/gitconfig), mark the active working directory safe for git and libgit2 operations.
if [ "${DISCIPLINE_TRUST_WORKSPACE:-}" = "1" ] && [ "$(pwd)" != "/workspace" ]; then
    git config --global --add safe.directory "$(pwd)" 2>/dev/null || true
fi

# If the first argument is a flag or empty, prepend `discipline`
if [ "$#" -eq 0 ] || [ "${1#-}" != "$1" ]; then
    exec discipline "$@"
elif [ "$1" = "discipline" ]; then
    exec "$@"
elif ! command -v "$1" >/dev/null 2>&1; then
    # Not a program on PATH: a discipline subcommand (check, doctor, baseline, bench, ...).
    # Deciding by exclusion keeps new subcommands working without editing this list.
    exec discipline "$@"
else
    # Execute arbitrary shell commands (e.g. `sh -c '...'` for in-container pipelines)
    exec "$@"
fi
