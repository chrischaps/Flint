#!/usr/bin/env bash
# The changelog for games built on Flint (#765, #788).
#
# An entry starts as a fragment, changelog.d/<issue>-<slug>.md, one file per
# pull request, so two PRs never write the same file. After the merge, the
# `Changelog` workflow assembles the fragments into one file a month,
# changelog/YYYY-MM.md, newest first, and deletes them. CHANGELOG.md stays a
# short page that links the months.
#
#   scripts/changelog.sh new <issue> <slug>    write changelog.d/<issue>-<slug>.md to fill in
#   scripts/changelog.sh since <YYYY-MM-DD>    every entry dated that day or later, newest
#                                              first, fragments included (also --since <date>)
#   scripts/changelog.sh assemble [--dry-run]  move the fragments into their month files
#   scripts/changelog.sh check                 the fragments are well formed
#   scripts/changelog.sh count                 entries in the month files and the fragments
#
# A fragment's date is the day of the commit that brought it onto the current
# branch's first-parent line: on main, the merge; in a game that carries the
# engine as a subtree, the pull that brought it. One not yet committed is
# dated today. Plain bash, git and POSIX awk, so a game can run it from
# engine/scripts/ on any machine that has git.
set -euo pipefail

cd "$(dirname "$0")/.."

frag_dir=changelog.d
month_dir=changelog
tab=$(printf '\t')
date_re='^[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]$'

die() { echo "changelog: $*" >&2; exit 2; }

# Every fragment, in name order (README.md explains the folder and isn't one).
fragment_files() {
    local f
    [ -d "$frag_dir" ] || return 0
    for f in "$frag_dir"/*.md; do
        [ -e "$f" ] || continue
        [ "$(basename "$f")" = README.md ] && continue
        printf '%s\n' "$f"
    done
}

# "<unix time> <YYYY-MM-DD>" for a fragment: the first-parent commit that
# added it, or now when it isn't committed (or there is no git).
stamp() {
    local s=""
    # --diff-merges can bring a patch with it; keep only the marked line.
    s=$(git log --first-parent --diff-merges=first-parent --diff-filter=A -1 \
        --format='changelog-stamp %ct %cs' -- "$1" 2>/dev/null | sed -n 's/^changelog-stamp //p' || true)
    [ -n "$s" ] || s="$(date +%s) $(date +%Y-%m-%d)"
    printf '%s\n' "$s"
}

# Fragments, newest first, as "<unix time> <date> <path>" lines.
stamped_fragments() {
    local f
    fragment_files | while IFS= read -r f; do
        printf '%s %s\n' "$(stamp "$f")" "$f"
    done | LC_ALL=C sort -k1,1nr -k3,3
}

# Splits a file into entry records, one line each:
#   <date> TAB <entry order> TAB <line no> TAB <text>
# An entry runs from its "### " line to the next "### " or "## <date>" line,
# trailing blank lines dropped. With fixed_date set (a fragment), date
# headings aren't read and every entry takes that date. Text between a date
# heading and its first entry would be lost, so it is an error.
records() { # file base [fixed_date]
    awk -v base="$2" -v fixed="${3:-}" -v file="$1" '
        function flush(   i) {
            while (n > 0 && lines[n] ~ /^[ \t]*$/) n--
            for (i = 1; i <= n; i++)
                printf "%s\t%07d\t%05d\t%s\n", date, base + e, i, lines[i]
            n = 0
        }
        BEGIN { date = fixed }
        { sub(/\r$/, "") }
        fixed == "" && /^## [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9][ \t]*$/ {
            flush(); inentry = 0; date = $2; next
        }
        /^### / {
            if (date == "") { printf "changelog: %s:%d: an entry before any date heading\n", file, NR > "/dev/stderr"; bad = 1; exit }
            flush(); e++; inentry = 1
        }
        inentry { lines[++n] = $0; next }
        date != "" && !/^[ \t]*$/ {
            printf "changelog: %s:%d: text outside an entry: %s\n", file, NR, $0 > "/dev/stderr"; bad = 1; exit
        }
        END { if (!bad) flush(); exit bad }
    ' "$1"
}

# Records, sorted newest first, back to markdown: a "## <date>" heading per
# day, entries in their order, one blank line between.
emit() {
    LC_ALL=C sort -t "$tab" -k1,1r -k2,2n -k3,3n | awk -F '\t' '
        {
            text = $0; sub(/^[^\t]*\t[^\t]*\t[^\t]*\t/, "", text)
            if ($1 != d) { if (NR > 1) print ""; print "## " $1; print ""; d = $1; o = $2 }
            else if ($2 != o) { print ""; o = $2 }
            print text
        }'
}

# The lines of a month file before its first date heading.
month_header() {
    awk '{ sub(/\r$/, "") } /^## [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9][ \t]*$/ { exit } { print }' "$1" |
        awk '{ l[NR] = $0 } END { n = NR; while (n > 0 && l[n] ~ /^[ \t]*$/) n--; for (i = 1; i <= n; i++) print l[i] }'
}

month_name() { # YYYY-MM -> "November 2026"
    local names=(January February March April May June July August September October November December)
    local m=$((10#${1#*-}))
    echo "${names[m - 1]} ${1%-*}"
}

new_month_header() { # YYYY-MM
    echo "# Changelog: $(month_name "$1")"
    echo
    echo "What changed this month for games built on Flint, newest first. How to read"
    echo "it and how to add an entry: [CHANGELOG.md](../CHANGELOG.md)."
}

# Every record there is: the month files, then the fragments.
all_records() {
    local f i=0 t d
    if [ -d "$month_dir" ]; then
        for f in "$month_dir"/[0-9]*.md; do
            [ -e "$f" ] || continue
            records "$f" 1000000
        done
    fi
    while read -r t d f; do
        records "$f" $((i * 100)) "$d"
        i=$((i + 1))
    done < <(stamped_fragments)
}

cmd_since() {
    local since=${1:-}
    [[ $since =~ $date_re ]] || die "since: give a date as YYYY-MM-DD (the day of your last engine pull)"
    local out
    out=$(all_records | awk -F '\t' -v s="$since" '$1 >= s' | emit)
    if [ -z "$out" ]; then
        echo "changelog: no entries dated $since or later" >&2
        return 0
    fi
    printf '%s\n' "$out"
    printf '%s\n' "$out" | awk -v s="$since" '/^### /{n++} END { printf "changelog: %d entries dated %s or later\n", n, s > "/dev/stderr" }'
}

cmd_count() {
    all_records | cut -f1,2 | LC_ALL=C sort -u | wc -l | tr -d ' '
}

cmd_check() {
    local f name bad=0 words
    while IFS= read -r f; do
        name=$(basename "$f")
        if ! [[ $name =~ ^[0-9]+-[a-z0-9][a-z0-9-]*\.md$ ]]; then
            echo "changelog: $f: name it <issue>-<slug>.md (lower case, digits and dashes)" >&2; bad=1
        fi
        if [ "$(awk 'NF { print; exit }' "$f" | cut -c1-4)" != '### ' ]; then
            echo "changelog: $f: start with the entry's '### <title> (#<issue>)' line" >&2; bad=1
        fi
        if grep -q '^## ' "$f"; then
            echo "changelog: $f: no '## ' lines: the date heading is added when it is assembled" >&2; bad=1
        fi
        words=$(wc -w < "$f" | tr -d ' ')
        if [ "$words" -gt 250 ]; then
            echo "changelog: $f: $words words; an entry reads best under about 150 (see CHANGELOG.md, \"Adding an entry\")" >&2
        fi
        records "$f" 0 2000-01-01 >/dev/null || bad=1
    done < <(fragment_files)
    [ "$bad" = 0 ] && echo "changelog: fragments well formed"
    return "$bad"
}

# CHANGELOG.md's list of months, newest first, between its "months" markers.
write_index() {
    local f m list="" tmp
    [ -f CHANGELOG.md ] || return 0
    for f in $(ls "$month_dir"/[0-9]*.md 2>/dev/null | LC_ALL=C sort -r); do
        m=$(basename "$f" .md)
        list="$list- [$(month_name "$m")]($month_dir/$m.md)"$'\n'
    done
    tmp=$(mktemp)
    awk -v list="$list" '
        { sub(/\r$/, "") }
        /^<!-- \/months/ { skip = 0 }
        !skip { print }
        /^<!-- months/ { printf "%s", list; skip = 1 }
    ' CHANGELOG.md > "$tmp"
    mv "$tmp" CHANGELOG.md
}

cmd_assemble() {
    local dry=0
    [ "${1:-}" = --dry-run ] && dry=1
    cmd_check >/dev/null || die "assemble: fix the fragments first (scripts/changelog.sh check)"
    local stamped
    stamped=$(stamped_fragments)
    if [ -z "$stamped" ]; then
        echo "changelog: no fragments to assemble"
        return 0
    fi
    local before after month out tmp t d f i
    before=$(cmd_count)
    mkdir -p "$month_dir"
    for month in $(printf '%s\n' "$stamped" | awk '{ print substr($2, 1, 7) }' | LC_ALL=C sort -u); do
        out="$month_dir/$month.md"
        tmp=$(mktemp)
        {
            if [ -f "$out" ]; then month_header "$out"; else new_month_header "$month"; fi
            echo
            {
                if [ -f "$out" ]; then records "$out" 1000000; fi
                i=0
                printf '%s\n' "$stamped" | while read -r t d f; do
                    if [ "${d:0:7}" = "$month" ]; then records "$f" $((i * 100)) "$d"; fi
                    i=$((i + 1))
                done
            } | emit
        } > "$tmp"
        if [ "$dry" = 1 ]; then
            echo "would write $out:"
            diff -u "$out" "$tmp" 2>/dev/null || diff -u /dev/null "$tmp" || true
            rm -f "$tmp"
        else
            mv "$tmp" "$out"
            echo "changelog: wrote $out"
        fi
    done
    [ "$dry" = 1 ] && return 0
    write_index
    printf '%s\n' "$stamped" | while read -r t d f; do
        rm -f "$f"
        echo "changelog: assembled $f ($d)"
    done
    after=$(cmd_count)
    [ "$before" = "$after" ] || die "assemble: $before entries before, $after after; nothing should be lost"
    echo "changelog: $after entries"
}

cmd_new() {
    local issue=${1:-} slug=${2:-}
    [[ $issue =~ ^[0-9]+$ && $slug =~ ^[a-z0-9][a-z0-9-]*$ ]] ||
        die "new: give an issue number and a slug: scripts/changelog.sh new 788 changelog-by-month"
    local f="$frag_dir/$issue-$slug.md"
    [ -e "$f" ] && die "new: $f exists"
    mkdir -p "$frag_dir"
    {
        echo "### <what a game author sees, naming the key, function or flag> (#$issue)"
        echo "**What changed.** <what is different, and what happens to old files and scripts>"
        echo "**What to do.** <the step to take, or \"Nothing.\">"
    } > "$f"
    echo "$f"
}

case "${1:-}" in
    new) shift; cmd_new "$@" ;;
    since | --since) shift; cmd_since "$@" ;;
    assemble) shift; cmd_assemble "$@" ;;
    check) cmd_check ;;
    count) cmd_count ;;
    *)
        sed -n '2,/^set -euo/p' "$0" | sed '$d' | sed 's/^# \{0,1\}//'
        [ -z "${1:-}" ] || exit 2
        ;;
esac
