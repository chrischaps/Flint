#!/usr/bin/env bash
# A change a game can notice needs a line in CHANGELOG.md (#765).
#
#   scripts/changelog-check.sh [base]      base defaults to origin/main
#
# Looks at what this branch changes since it left <base>. If it touches a
# watched path (the engine's schemas, the script API, the scene format) and
# not CHANGELOG.md, it fails, unless a commit message on the branch carries a
# line of its own:
#
#   Changelog: none (reason)
#
# for a change no game can notice (a comment, a refactor, a test). CI's
# `changelog` job runs this on every pull request.
#
# CLI flags, Cargo features, file formats and defaults aren't watched: no path
# says reliably that one changed. They still belong in the changelog.
set -euo pipefail

cd "$(dirname "$0")/.."
base=${1:-origin/main}

watched='^(schemas/|crates/flint-script/src/api(\.rs|/)|crates/flint-scene/src/format\.rs$)'

if ! git rev-parse -q --verify "$base" >/dev/null 2>&1; then
    echo "changelog: no such base '$base' (fetch it, or name another)" >&2
    exit 2
fi

changed=$(git diff --name-only "$base"...HEAD)
hits=$(printf '%s\n' "$changed" | grep -E "$watched" || true)

if [ -z "$hits" ]; then
    echo "changelog: nothing a game reads changed"
    exit 0
fi
if printf '%s\n' "$changed" | grep -qx 'CHANGELOG.md'; then
    echo "changelog: entry present"
    exit 0
fi
if git log --format=%B "$base"..HEAD | grep -qiE '^Changelog: none \(.+\)'; then
    echo "changelog: waived by a commit message:"
    git log --format=%B "$base"..HEAD | grep -iE '^Changelog: none \(.+\)' | sed 's/^/  /'
    exit 0
fi

{
    echo "changelog: this branch changes what a game reads, and CHANGELOG.md has no entry:"
    printf '%s\n' "$hits" | sed 's/^/  /'
    echo "Add an entry to CHANGELOG.md (see its \"Adding an entry\"), or, if no game can"
    echo "notice the change, put this line in a commit message on the branch:"
    echo "  Changelog: none (reason)"
} >&2
exit 1
