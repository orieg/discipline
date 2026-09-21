#!/bin/sh
set -e

# In ephemeral container environments, ensure the working directory is explicitly marked safe
# for git and libgit2 operations, covering custom mount paths (e.g. GitLab CI /builds/... or Argo).
git config --global --add safe.directory "$(pwd)" 2>/dev/null || true

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
