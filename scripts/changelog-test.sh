#!/usr/bin/env bash
# Tests scripts/changelog.sh and scripts/changelog-check.sh (#788) in a
# throwaway repository holding this checkout's changelog:
#
#   bash scripts/changelog-test.sh
#
# Two branches that each add an entry merge in either order, and across an
# assembly, with no conflict; `since` prints exactly the entries dated that
# day or later, fragments included, and the same before and after assembly;
# assembly loses no entry and lists the new month; the check fails a schema
# change with no fragment and passes one with a fragment or a waiver.
set -euo pipefail

src=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
pass=0
fail=0
ok() { echo "ok   $*"; pass=$((pass + 1)); }
no() { echo "FAIL $*"; fail=$((fail + 1)); }
check() { if eval "$2"; then ok "$1"; else no "$1"; fi; }

export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
at() { # at <YYYY-MM-DD> <git args...>: a commit made on that day
    local d=$1; shift
    GIT_AUTHOR_DATE="${d}T12:00:00Z" GIT_COMMITTER_DATE="${d}T12:00:00Z" git "$@"
}

repo=$work/engine
mkdir -p "$repo/scripts" "$repo/schemas" "$repo/changelog.d"
cp "$src/scripts/changelog.sh" "$src/scripts/changelog-check.sh" "$repo/scripts/"
cp -r "$src/changelog" "$repo/"
cp "$src/CHANGELOG.md" "$src/changelog.d/README.md" "$repo/" 2>/dev/null || true
mv "$repo/README.md" "$repo/changelog.d/README.md"
echo 'x = 1' > "$repo/schemas/a.toml"
echo 'y = 1' > "$repo/schemas/b.toml"
cd "$repo"
git init -q -b main
git config core.autocrlf false
git add -A
at 2026-11-20 commit -qm base
start=$(bash scripts/changelog.sh count)

entry() { # entry <issue> <title>
    printf '### %s (#%s)\n**What changed.** %s changed.\n**What to do.** Nothing.\n' "$2" "$1" "$2" > "changelog.d/$1-t$1.md"
}
branch() { # branch <name> <issue> <schema file>: a schema change with its fragment
    git checkout -q -b "$1" main
    echo "z = $2" >> "schemas/$3"
    entry "$2" "Change $2"
    git add -A
    at 2026-11-21 commit -qm "change $2"
    git checkout -q main
}
merge() { # merge <branch> <day>: a squash merge, as GitHub does it
    git merge -q --squash "$1" >/dev/null 2>&1 || return 1
    at "$2" commit -qm "merge $1"
}

branch a 901 a.toml
branch b 902 b.toml

# --- the check ---
git checkout -q a
check "check passes a schema change with a fragment" "bash scripts/changelog-check.sh main >/dev/null 2>&1"
git checkout -q -b c main
echo 'z = 3' >> schemas/a.toml
git add -A
at 2026-11-21 commit -qm "no entry"
check "check fails a schema change with no fragment" "! bash scripts/changelog-check.sh main >/dev/null 2>&1"
at 2026-11-21 commit -q --allow-empty -m "waive" -m "Changelog: none (a comment)"
check "check passes it with a waiver" "bash scripts/changelog-check.sh main >/dev/null 2>&1"
git checkout -q -b d main
echo 'z = 4' >> schemas/a.toml
printf 'no heading\n' > changelog.d/904-bad.md
git add -A
at 2026-11-21 commit -qm "bad fragment"
check "check fails a fragment with no ### line" "! bash scripts/changelog-check.sh main >/dev/null 2>&1"
git checkout -q -b e main
cat changelog/2026-11.md >> CHANGELOG.md
git add -A
at 2026-11-21 commit -qm "entries in the index"
check "check fails a CHANGELOG.md over 3 KB" "! bash scripts/changelog-check.sh main >/dev/null 2>&1"
git checkout -q main

# --- merges in either order ---
git checkout -q -b ab main
check "a then b merge with no conflict" "merge a 2026-12-02 && merge b 2026-12-05"
git checkout -q -b ba main
check "b then a merge with no conflict" "merge b 2026-12-02 && merge a 2026-12-05"
git checkout -q -b nf main
at 2026-12-09 merge -q --no-ff a -m "merge commit a"
check "a merge commit dates the entry by the merge, not the branch's commit" \
    "bash scripts/changelog.sh since 2026-12-09 2>/dev/null | grep -q '^## 2026-12-09'"
git checkout -q main
check "a, an assembly, then b (branched before it) merge with no conflict" \
    "merge a 2026-12-02 && bash scripts/changelog.sh assemble >/dev/null && git add -A && at 2026-12-02 commit -qm assemble && merge b 2026-12-05"
git reset -q --hard ab

# --- since, fragments only ---
check "since 2026-12-03 is the one entry merged after it" \
    "[ \"\$(bash scripts/changelog.sh since 2026-12-03 2>/dev/null | grep -c '^### ')\" = 1 ] && bash scripts/changelog.sh since 2026-12-03 2>/dev/null | grep -q '^### Change 902'"
check "since 2026-12-02 includes that day's merge" \
    "[ \"\$(bash scripts/changelog.sh since 2026-12-02 2>/dev/null | grep -c '^### ')\" = 2 ]"
check "since 2026-12-06 prints nothing" "[ -z \"\$(bash scripts/changelog.sh since 2026-12-06 2>/dev/null)\" ]"
check "since is newest first" \
    "bash scripts/changelog.sh since 2026-12-01 2>/dev/null | grep '^## ' | tr '\n' ' ' | grep -q '^## 2026-12-05 ## 2026-12-02 \$'"
real_days=$(awk '/^## 2026-11-1[01]/ { on = 1; next } /^## / { on = 0 } on && /^### /' changelog/2026-11.md | wc -l | tr -d ' ')
check "since 2026-11-10 prints the old entries of the 10th and 11th and the two new ones ($real_days + 2)" \
    "[ \"\$(bash scripts/changelog.sh since 2026-11-10 2>/dev/null | grep -c '^### ')\" = $((real_days + 2)) ]"
before_since=$(bash scripts/changelog.sh since 2026-11-01 2>/dev/null)

# --- assembly ---
check "assemble succeeds" "bash scripts/changelog.sh assemble >/dev/null"
check "assembly loses no entry ($start + 2)" "[ \"\$(bash scripts/changelog.sh count)\" = $((start + 2)) ]"
check "assembly removes the fragments and keeps the README" \
    "[ \"\$(ls changelog.d)\" = README.md ]"
check "assembly writes December's file, dated by the merges" \
    "grep -q '^## 2026-12-05' changelog/2026-12.md && grep -q '^## 2026-12-02' changelog/2026-12.md && grep -q '^# Changelog: December 2026' changelog/2026-12.md"
check "CHANGELOG.md lists December above November" \
    "grep -n '^- \\[' CHANGELOG.md | tr '\n' ' ' | grep -q 'December 2026.*November 2026'"
check "CHANGELOG.md stays under 3 KB" "[ \$(wc -c < CHANGELOG.md) -le 3072 ]"
check "since prints the same before and after assembly" \
    "[ \"\$(bash scripts/changelog.sh since 2026-11-01 2>/dev/null)\" = \"\$before_since\" ]"
check "November's file is untouched by December's assembly" \
    "diff -q changelog/2026-11.md '$src/changelog/2026-11.md' >/dev/null"
check "a second assembly does nothing" "bash scripts/changelog.sh assemble | grep -q 'no fragments'"

# --- a same-month assembly keeps the day's older entries below the new ---
git add -A
at 2026-12-05 commit -qm assemble
entry 905 "Later the same day"
git add -A
at 2026-12-05 commit -qm "merge 905"
bash scripts/changelog.sh assemble >/dev/null
check "a later entry on an assembled day goes above that day's earlier one" \
    "awk '/^### /' changelog/2026-12.md | head -2 | tr '\n' ' ' | grep -q 'Later the same day.*Change 902'"
check "and the day has one heading" "[ \"\$(grep -c '^## 2026-12-05' changelog/2026-12.md)\" = 1 ]"

echo
echo "$pass passed, $fail failed"
[ "$fail" = 0 ]
