#!/usr/bin/env bash
# Every ADR the tree cites has a record in docs/adr (#374).
#
#   scripts/adr-check.sh          exit 1 naming each cited number with no file
#   scripts/adr-check.sh --list   print each cited number and the file it resolves to
#
# A citation is "ADR NNNN" or "ADRs NNNN", in any tracked file, including the
# compound forms the tree uses: "ADRs 0071-0075" (every number in the range),
# "ADR 0006/0014", "ADR 0082, 0097", "ADR 0058 / 0048". A number resolves when
# docs/adr/NNNN-<slug>.md exists. `scripts/ci.sh` runs this, so CI's rustfmt
# job and `issue.sh land` do too. Seconds, no build.
set -euo pipefail

cd "$(dirname "$0")/.."

list=0
[ "${1:-}" = "--list" ] && list=1

# Each citation, compound forms whole: "ADRs 0071-0075", "ADR 0082, 0097".
cites=$(git grep -ohE "ADRs? ?[0-9]{4}( ?([-–/,]|and) ?[0-9]{4})*" -- . || true)

# Expand to one number per line: a dash between two numbers is a range.
numbers=$(printf '%s\n' "$cites" | sed -E 's/^ADRs? ?//; s/ ?(–|-) ?/-/g; s/ ?(\/|,|and) ?/ /g' |
    tr ' ' '\n' | awk -F- 'NF == 2 { for (i = $1 + 0; i <= $2 + 0; i++) printf "%04d\n", i; next }
                           NF == 1 && $1 != "" { printf "%04d\n", $1 + 0 }' | sort -u)

missing=()
for n in $numbers; do
    file=$(ls docs/adr/"$n"-*.md 2>/dev/null | head -1 || true)
    if [ -z "$file" ]; then
        missing+=("$n")
    elif [ "$list" = 1 ]; then
        echo "$n $file"
    fi
done

if [ "${#missing[@]}" -gt 0 ]; then
    echo "adr-check: cited with no record in docs/adr: ${missing[*]}" >&2
    for n in "${missing[@]}"; do
        git grep -nE "ADRs? ?([0-9]{4}[^A-Za-z]{0,4})*$n" -- . | head -3 | sed 's/^/  /' >&2 || true
    done
    echo "adr-check: write docs/adr/NNNN-<slug>.md (see docs/adr/README.md) or fix the citation" >&2
    exit 1
fi
[ "$list" = 1 ] || echo "adr-check: $(printf '%s\n' $numbers | wc -l | tr -d ' ') cited ADRs, each has a record"
