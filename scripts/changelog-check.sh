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
# branch it also checks that the fragments are well formed, that CHANGELOG.md
# stays a short page (the entries live in changelog/), and that no fragment the
# branch adds has a heading a month file already holds: one a merge brought
# back after its assembly (#1006). CI's `changelog` job runs this on every
# pull request.
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

fragments=$(git diff --name-only --diff-filter=AM "$base"...HEAD | grep -E "$fragment" | grep -v '/README\.md$' || true)

# A fragment this branch adds whose entry a month file already holds came back
# with a merge (#1006): a branch stacked on a sibling's carries the sibling's
# fragment as its own addition, and squashing it onto main after the sibling's
# assembly adds it again. assemble deletes one that is word for word what the
# month holds (#994); this catches it before the merge, by its heading, so an
# older copy of the sibling's text is caught too, and so it doesn't pass for
# this branch's own entry. The months are base's and this branch's: a stacked
# branch that hasn't merged main has the fragment and not the assembly.
assembled_headings() {
    local f
    git ls-tree --name-only "$base" -- changelog/ | grep -E '^changelog/[0-9][^/]*\.md$' |
        while IFS= read -r f; do git show "$base:$f"; done
    for f in changelog/[0-9]*.md; do
        [ -e "$f" ] && cat "$f"
    done
    return 0
}
if [ -n "$fragments" ]; then
    headings=$(assembled_headings | sed 's/\r$//' | grep '^### ' | LC_ALL=C sort -u || true)
    returned=""
    while IFS= read -r f; do
        [ -f "$f" ] || continue
        while IFS= read -r h; do
            if printf '%s\n' "$headings" | grep -qxF -- "$h"; then
                returned="$returned  $f: $h"$'\n'
            fi
        done < <(sed 's/\r$//' "$f" | grep '^### ' || true)
    done <<< "$fragments"
    if [ -n "$returned" ]; then
        {
            echo "changelog: this branch adds a fragment whose entry is already in changelog/:"
            printf '%s' "$returned"
            echo "A merge brought it back after it was assembled: this branch is stacked on"
            echo "another's, or merged it before its assembly. Drop it (git rm <file>) and"
            echo "commit. If it is a new entry, give it a heading of its own."
        } >&2
        exit 1
    fi
fi

changed=$(git diff --name-only "$base"...HEAD)
hits=$(printf '%s\n' "$changed" | grep -E "$watched" || true)

if [ -z "$hits" ]; then
    echo "changelog: nothing a game reads changed"
    exit 0
fi
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
