#!/usr/bin/env bash
# Tests scripts/changelog.sh and scripts/changelog-check.sh (#788) in a
# throwaway repository holding this checkout's changelog:
#
#   bash scripts/changelog-test.sh
#
# Two branches that each add an entry merge in either order, and across an
# assembly, with no conflict; `since` prints exactly the entries dated that
# day or later, fragments included, and the same before and after assembly;
# assembly loses no entry and lists the new month; a fragment a squash merge
# brings back after its assembly is neither printed nor assembled twice, and
# assembling it deletes it (#994), and the check fails a branch that carries
# it before the merge, by its heading (#1006); the check fails a schema change
# with no fragment and passes one with a fragment or a waiver. Packages
# (#1564): `new --package` writes the package line and refuses a name that
# isn't a package; assembly files a day's entries under one heading per
# package, the kernel's first, and a returned package fragment is still
# assembled once; `since --package` prints only those packages' entries; the
# check notes an entry filed under engine on a branch that changes only one
# package's folder.
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
for p in engine living music; do
    mkdir -p "$repo/packages/$p"
    printf '[package]\nname = "%s"\n' "$p" > "$repo/packages/$p/package.toml"
done
echo 'x = 1' > "$repo/schemas/a.toml"
echo 'y = 1' > "$repo/schemas/b.toml"
cd "$repo"
git init -q -b main
git config core.autocrlf false
git add -A
at 2026-11-20 commit -qm base
start=$(bash scripts/changelog.sh count)

entry() { # entry <issue> <title> [package]: no package line without one
    {
        printf '### %s (#%s)\n' "$2" "$1"
        [ -z "${3:-}" ] || printf '**Package.** %s\n' "$3"
        printf '**What changed.** %s changed.\n**What to do.** Nothing.\n' "$2"
    } > "changelog.d/$1-t$1.md"
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
real_days=$(awk '/^## / { on = ($2 >= "2026-11-10") } on && /^### /' changelog/2026-11.md | wc -l | tr -d ' ')
check "since 2026-11-10 prints the old entries of the 10th on and the two new ones ($real_days + 2)" \
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

# --- a fragment a merge brings back after its assembly (#994) ---
# s2 and s3 are stacked on s1, so they hold s1's fragment as their own
# addition: squashed onto main after s1's assembly, each adds it back.
git add -A
at 2026-12-05 commit -qm assemble
git checkout -q -b s1 main
echo 'z = 906' >> schemas/a.toml
entry 906 "Stacked under two branches"
git add -A
at 2026-12-06 commit -qm "change 906"
git checkout -q -b s2 s1
echo 'readme' > notes.txt
git add -A
at 2026-12-06 commit -qm "no entry of its own"
git checkout -q -b s3 s1
echo 'z = 908' >> schemas/b.toml
entry 908 "Change 908"
git add -A
at 2026-12-06 commit -qm "change 908"
git checkout -q main
merge s1 2026-12-07
bash scripts/changelog.sh assemble >/dev/null
git add -A
at 2026-12-07 commit -qm assemble
assembled=$(cat changelog/2026-12.md)
count=$(bash scripts/changelog.sh count)

# --- the check catches it before the merge (#1006) ---
git checkout -q s3
check "while s3 targets s1, the check passes it" "bash scripts/changelog-check.sh s1 >/dev/null 2>&1"
check "against main after s1's assembly, the check fails s3 for carrying s1's fragment" \
    "! bash scripts/changelog-check.sh main >/dev/null 2>&1"
said=$(bash scripts/changelog-check.sh main 2>&1 || true)
check "and names the fragment and its heading" \
    "printf '%s\n' \"\$said\" | grep -qF 'changelog.d/906-t906.md: ### Stacked under two branches (#906)'"
check "and doesn't name s3's own fragment" \
    "! printf '%s\n' \"\$said\" | grep -q '908-t908'"
git checkout -q s2
check "s2 changes nothing a game reads, and still fails for carrying it" \
    "! bash scripts/changelog-check.sh main >/dev/null 2>&1"
git checkout -q -b s2-crlf s2
sed 's/$/\r/' changelog.d/906-t906.md > changelog.d/x && mv changelog.d/x changelog.d/906-t906.md
printf 'One more line.\r\n' >> changelog.d/906-t906.md
git add -A
at 2026-12-06 commit -qm "an older copy, CRLF"
check "a copy whose text differs, with CRLF line ends, is caught by its heading" \
    "! bash scripts/changelog-check.sh main >/dev/null 2>&1"
git checkout -q -b s3-fixed s3
git rm -q changelog.d/906-t906.md
at 2026-12-06 commit -qm "drop s1's fragment"
check "once s3 drops it, the check passes s3 on its own entry" \
    "bash scripts/changelog-check.sh main 2>/dev/null | grep -q '908-t908'"
git checkout -q s2
git rm -q changelog.d/906-t906.md
at 2026-12-06 commit -qm "drop s1's fragment"
# s2's diff from main still holds s1's schema change, so it is judged on that
# (no entry of its own), no longer on the returned fragment.
said=$(bash scripts/changelog-check.sh main 2>&1 || true)
check "and s2, dropped, is no longer failed for a returned fragment" \
    "! printf '%s\n' \"\$said\" | grep -q 'already in changelog/' && printf '%s\n' \"\$said\" | grep -q 'adds no changelog entry'"
git reset -q --hard HEAD~1
git checkout -q main
git branch -q -D s2-crlf s3-fixed

merge s2 2026-12-08
check "squashing a branch stacked on an assembled one brings its fragment back" "[ -f changelog.d/906-t906.md ]"
check "since prints the returned entry once" \
    "[ \"\$(bash scripts/changelog.sh since 2026-12-01 2>/dev/null | grep -c '^### Stacked under two branches')\" = 1 ]"
check "count doesn't count it again" "[ \"\$(bash scripts/changelog.sh count)\" = $count ]"
check "assembling it succeeds" "bash scripts/changelog.sh assemble >/dev/null"
check "and leaves the month unchanged" "[ \"\$(cat changelog/2026-12.md)\" = \"\$assembled\" ]"
check "and deletes the fragment" "[ \"\$(ls changelog.d)\" = README.md ]"
git add -A
at 2026-12-08 commit -qm assemble
merge s3 2026-12-09
check "beside a new fragment, the returned one isn't added again" \
    "bash scripts/changelog.sh assemble >/dev/null && [ \"\$(grep -c '^### Stacked under two branches' changelog/2026-12.md)\" = 1 ]"
check "and the new one is" "[ \"\$(grep -c '^### Change 908' changelog/2026-12.md)\" = 1 ] && [ \"\$(bash scripts/changelog.sh count)\" = $((count + 1)) ]"
git add -A
at 2026-12-09 commit -qm assemble

# --- packages (#1564) ---
check "new --package living writes a fragment naming living, after its title" \
    "bash scripts/changelog.sh new 1 demo --package living >/dev/null && sed -n 2p changelog.d/1-demo.md | grep -qxF '**Package.** living'"
said=$(bash scripts/changelog.sh new 2 demo --package nope 2>&1 && echo "exit=0" || echo "exit=$?")
check "new --package nope is refused (exit 2), naming the packages there are" \
    "printf '%s\n' \"\$said\" | grep -qF \"no package 'nope'; the packages are: engine, living, music\" && printf '%s\n' \"\$said\" | grep -qx 'exit=2' && [ ! -e changelog.d/2-demo.md ]"
check "new with no --package files the entry under engine" \
    "bash scripts/changelog.sh new 3 demo >/dev/null && sed -n 2p changelog.d/3-demo.md | grep -qxF '**Package.** engine'"
check "check passes the two" "bash scripts/changelog.sh check >/dev/null 2>&1"
rm changelog.d/1-demo.md changelog.d/3-demo.md
entry 913 "Misnamed" nope
check "check fails a package line naming no package" "! bash scripts/changelog.sh check >/dev/null 2>&1"
printf '### Late (#914)\n**What changed.** Late.\n**Package.** living\n' > changelog.d/914-t914.md
rm changelog.d/913-t913.md
said=$(bash scripts/changelog.sh check 2>&1 && echo "exit=0" || echo "exit=$?")
check "and one that isn't right after the title" \
    "printf '%s\n' \"\$said\" | grep -qF '914-t914.md:3: put the' && printf '%s\n' \"\$said\" | grep -qx 'exit=1'"
rm changelog.d/914-t914.md

git checkout -q -b pk main
echo 'x = 2' >> packages/living/package.toml
entry 918 "Living's own"
git add -A
at 2026-12-10 commit -qm "a living change filed under engine"
said=$(bash scripts/changelog-check.sh main 2>&1 || true)
check "the check notes a fragment filed under engine on a branch that changes only packages/living/" \
    "printf '%s\n' \"\$said\" | grep -qF 'changelog.d/918-t918.md is filed under engine, but this branch changes only packages/living/'"
entry 918 "Living's own" living
git add -A
at 2026-12-10 commit -qm "filed under living"
said=$(bash scripts/changelog-check.sh main 2>&1 || true)
check "and doesn't once it is filed under living" "! printf '%s\n' \"\$said\" | grep -q 'note:'"
echo 'y = 2' >> schemas/a.toml
entry 918 "Living's own"
git add -A
at 2026-12-10 commit -qm "and a kernel change"
said=$(bash scripts/changelog-check.sh main 2>&1 || true)
check "nor when the branch changes the kernel too" \
    "printf '%s\n' \"\$said\" | grep -q 'entry present' && ! printf '%s\n' \"\$said\" | grep -q 'note:'"
git checkout -q main
git branch -q -D pk

entry 910 "Living 910" living
entry 911 "Music 911" music
entry 912 "Kernel 912"
entry 915 "Kernel 915" engine
git add -A
at 2026-12-10 commit -qm "merge 910-915"
before_since=$(bash scripts/changelog.sh since 2026-12-10 2>/dev/null)
check "since --package living prints living's entry alone, under its heading" \
    "[ \"\$(bash scripts/changelog.sh since 2026-12-10 --package living 2>/dev/null | grep -E '^##')\" = \"\$(printf '## 2026-12-10 \\302\\267 living\n### Living 910 (#910)')\" ]"
check "since --package engine,music prints the kernel's two and music's" \
    "[ \"\$(bash scripts/changelog.sh since 2026-12-10 --package engine,music 2>/dev/null | grep -c '^### ')\" = 3 ] && ! bash scripts/changelog.sh since 2026-12-10 --package engine --package music 2>/dev/null | grep -q Living"
check "since --package nope is refused" "! bash scripts/changelog.sh since 2026-12-10 --package nope >/dev/null 2>&1"
count=$(bash scripts/changelog.sh count)
said=$(bash scripts/changelog.sh assemble)
check "assembly says it assembled each of the four, none 'already in changelog/'" \
    "[ \"\$(printf '%s\n' \"\$said\" | grep -c '^changelog: assembled changelog.d/91[0-5]-')\" = 4 ] && ! printf '%s\n' \"\$said\" | grep -q 'already in'"
day=$(awk '/^## 2026-12-10/ { on = 1 } /^## 2026-12-0/ { on = 0 } on && /^##/' changelog/2026-12.md)
check "assembly files each entry under its package: the kernel's unnamed and first, then by name" \
    "[ \"\$day\" = \"\$(printf '## 2026-12-10\n### Kernel 912 (#912)\n### Kernel 915 (#915)\n## 2026-12-10 \\302\\267 living\n### Living 910 (#910)\n## 2026-12-10 \\302\\267 music\n### Music 911 (#911)')\" ]"
check "and drops the package lines from the entries" "! grep -q '^\*\*Package' changelog/2026-12.md"
check "and loses no entry" "[ \"\$(bash scripts/changelog.sh count)\" = $((count)) ]"
check "since prints the same after assembly" "[ \"\$(bash scripts/changelog.sh since 2026-12-10 2>/dev/null)\" = \"\$before_since\" ]"
git add -A
at 2026-12-10 commit -qm assemble
assembled=$(cat changelog/2026-12.md)
git show HEAD~1:changelog.d/910-t910.md > changelog.d/910-t910.md
git add -A
at 2026-12-11 commit -qm "a squash brings 910 back"
check "a returned package fragment is printed once" \
    "[ \"\$(bash scripts/changelog.sh since 2026-12-10 2>/dev/null | grep -c '^### Living 910')\" = 1 ]"
check "and assembling it leaves the month unchanged" \
    "bash scripts/changelog.sh assemble >/dev/null && [ \"\$(cat changelog/2026-12.md)\" = \"\$assembled\" ] && [ \"\$(ls changelog.d)\" = README.md ]"
git add -A
at 2026-12-11 commit -qm assemble
entry 916 "Living 916" living
git add -A
at 2026-12-10 commit -qm "merge 916, the same day"
bash scripts/changelog.sh assemble >/dev/null
check "a later entry of a package on an assembled day joins its heading, above the earlier" \
    "[ \"\$(grep -c '^## 2026-12-10 ' changelog/2026-12.md)\" = 2 ] && awk '/^## 2026-12-10 [^ ]+ living/ { on = 1; next } /^## / { on = 0 } on && /^### /' changelog/2026-12.md | tr '\n' ' ' | grep -q '^### Living 916.*### Living 910'"
nov_before=$(sed 's/\r$//' changelog/2026-11.md)
git add -A
at 2026-12-10 commit -qm assemble
entry 917 "Living in November" living
git add -A
at 2026-11-28 commit -qm "merge 917, dated in November"
bash scripts/changelog.sh assemble >/dev/null
check "assembling into a month written before packages changes only what it adds" \
    "[ \"\$(diff <(printf '%s\n' \"\$nov_before\") changelog/2026-11.md | grep -c '^<')\" = 0 ] && grep -q '^## 2026-11-28 [^ ]* living' changelog/2026-11.md"

echo
echo "$pass passed, $fail failed"
[ "$fail" = 0 ]
