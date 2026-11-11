#!/usr/bin/env bash
# A change a game can notice needs a changelog entry (#765): a fragment,
# changelog.d/<issue>-<slug>.md, one file per PR (#788).
#
#   scripts/changelog-check.sh [base]      base defaults to origin/main
#
# Looks at what this branch changes since it left <base>. If it touches a
# watched path (the engine's schemas, the script API, the scene format) and
# adds or edits no fragment, it fails, unless a commit message on the branch
# carries a line of its own:
#
#   Changelog: none (reason)
#
# for a change no game can notice (a comment, a refactor, a test). On every
# branch it also checks that the fragments are well formed and that
# CHANGELOG.md stays a short page (the entries live in changelog/). CI's
# `changelog` job runs this on every pull request.
#
# CLI flags, Cargo features, file formats and defaults aren't watched: no path
# says reliably that one changed. They still belong in the changelog.
set -euo pipefail

cd "$(dirname "$0")/.."
base=${1:-origin/main}

watched='^(schemas/|crates/flint-script/src/api(\.rs|/)|crates/flint-scene/src/format\.rs$)'
fragment='^changelog\.d/[^/]+\.md$'
index_limit=3072

if ! git rev-parse -q --verify "$base" >/dev/null 2>&1; then
    echo "changelog: no such base '$base' (fetch it, or name another)" >&2
    exit 2
fi

bash scripts/changelog.sh check

size=$(wc -c < CHANGELOG.md | tr -d ' ')
if [ "$size" -gt "$index_limit" ]; then
    {
        echo "changelog: CHANGELOG.md is $size bytes, over $index_limit. It is the page that"
        echo "links the months; an entry goes in changelog.d/ (scripts/changelog.sh new <issue> <slug>)."
    } >&2
    exit 1
fi

changed=$(git diff --name-only "$base"...HEAD)
hits=$(printf '%s\n' "$changed" | grep -E "$watched" || true)

if [ -z "$hits" ]; then
    echo "changelog: nothing a game reads changed"
    exit 0
fi
fragments=$(git diff --name-only --diff-filter=AM "$base"...HEAD | grep -E "$fragment" | grep -v '/README\.md$' || true)
if [ -n "$fragments" ]; then
    echo "changelog: entry present:"
    printf '%s\n' "$fragments" | sed 's/^/  /'
    exit 0
fi
if git log --format=%B "$base"..HEAD | grep -qiE '^Changelog: none \(.+\)'; then
    echo "changelog: waived by a commit message:"
    git log --format=%B "$base"..HEAD | grep -iE '^Changelog: none \(.+\)' | sed 's/^/  /'
    exit 0
fi

{
    echo "changelog: this branch changes what a game reads, and adds no changelog entry:"
    printf '%s\n' "$hits" | sed 's/^/  /'
    echo "Add one: bash scripts/changelog.sh new <issue> <slug>, fill in the file it names"
    echo "(see CHANGELOG.md, \"Adding an entry\"), and commit it. If no game can notice"
    echo "the change, put this line in a commit message on the branch:"
    echo "  Changelog: none (reason)"
} >&2
exit 1
