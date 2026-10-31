#!/usr/bin/env bash
# PreToolUse hook for Bash: refuses a `git commit` or `git push` of issue work
# when this session holds no claim on the issue. Claude Code passes the tool
# call as JSON on stdin; exit 2 denies it, and stderr is the reason the agent
# reads.
#
# Issue work is a branch named issue-<n> (or .../issue-<n>), or a commit
# message saying Closes/Fixes/Resolves #<n>. Anything else passes: a typo fix
# on another branch needs no claim. Commits are checked locally (the claim
# record, the worktree); pushes also ask origin that the claim is still ours.
#
# A push also reads the Closes lines of the commits it carries past the base
# branch. Two kinds of those aren't this session's work, and pass (#223):
# commits whose change is already on the base (a sibling rebased or
# cherry-picked in, or squashed from one commit), and commits naming an issue
# that's closed on GitHub, whose claim was reaped with it. That's what a branch
# stacked on a sibling's carries once the sibling squash-merges. The issue
# named by the branch, or by the commit being made, always counts.
#
# The canonical copy is skills/setup-claims in chrischaps/Dashboard.

set -uo pipefail

input=$(cat)

# A JSON string field, unescaped. Enough for the fields Claude Code sends.
field() {
  printf '%s' "$input" | tr -d '\r' | tr '\n' ' ' \
    | sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\(\([^\"\\\\]\|\\\\.\)*\)\".*/\1/p" \
    | sed 's/\\n/\n/g; s/\\t/\t/g; s/\\"/"/g; s/\\\\/\\/g'
}

cmd=$(field command)
cwd=$(field cwd)

commit="" push=""
grep -qE '(^|[;&|(]|\s)git(\s+-C\s+\S+)?\s+commit\b' <<<"$cmd" && commit=1
grep -qE '(^|[;&|(]|\s)git(\s+-C\s+\S+)?\s+push\b' <<<"$cmd" && push=1
[ -n "$commit$push" ] || exit 0
# Deleting a branch (the cleanup after a land) isn't issue work.
[ -z "$commit" ] && grep -qE '\s(--delete|-d)\s' <<<"$cmd" && exit 0

[ -n "$cwd" ] && cd "$cwd" 2>/dev/null
# `cd <dir> && git ...` and `git -C <dir> ...` run somewhere else.
dir=$(sed -nE 's/^\s*cd\s+("([^"]+)"|'"'"'([^'"'"']+)'"'"'|(\S+))\s*(&&|;).*/\2\3\4/p' <<<"$cmd" | head -1)
[ -z "$dir" ] && dir=$(sed -nE 's/.*git\s+-C\s+("([^"]+)"|(\S+))\s+(commit|push)\b.*/\2\3/p' <<<"$cmd" | head -1)
[ -n "$dir" ] && cd "$dir" 2>/dev/null

top=$(git rev-parse --show-toplevel 2>/dev/null) || exit 0
tool=$top/scripts/issue.sh
[ -f "$tool" ] || exit 0

claim_hint="" base=""
# shellcheck disable=SC1091
[ -f "$top/scripts/issue.conf" ] && . "$top/scripts/issue.conf"

closes() { grep -oiE '\b(close[sd]?|fix(e[sd])?|resolve[sd]?):?\s+#[0-9]+' | grep -oE '[0-9]+'; }

nums=()
branch=$(git branch --show-current 2>/dev/null)
[[ $branch =~ (^|/)issue-([0-9]+)$ ]] && nums+=("${BASH_REMATCH[2]}")

if [ -n "$commit" ]; then
  while read -r n; do [ -n "$n" ] && nums+=("$n"); done < <(closes <<<"$cmd")
  # git commit -F <file>: the message is in the file.
  file=$(sed -nE 's/.*\s(-F|--file)(\s+|=)("([^"]+)"|(\S+)).*/\4\5/p' <<<"$cmd" | head -1)
  if [ -n "$file" ] && [ "$file" != - ] && [ -f "$file" ]; then
    while read -r n; do [ -n "$n" ] && nums+=("$n"); done < <(closes <"$file")
  fi
fi
# The issues named only by the commits a push carries. They can be a
# sibling's, so a closed one passes below.
carried=() range=()
if [ -n "$push" ]; then
  remote=${CLAIM_REMOTE:-origin}
  b=${CLAIM_BASE:-$base}
  [ -n "$b" ] || b=$(git symbolic-ref -q --short "refs/remotes/$remote/HEAD" 2>/dev/null | sed "s#^$remote/##")
  for c in main master; do
    [ -n "$b" ] || ! git rev-parse -q --verify "$remote/$c" >/dev/null || b=$c
  done
  if [ -n "$b" ] && git rev-parse -q --verify "$remote/$b" >/dev/null; then
    # --cherry-pick leaves out the commits whose patch the base already has.
    range=("$remote/$b...HEAD" --cherry-pick --right-only)
    while read -r n; do
      [ -n "$n" ] || continue
      printf '%s\n' ${nums[@]+"${nums[@]}"} | grep -qx "$n" || carried+=("$n")
    done < <(git log "${range[@]}" --format=%B 2>/dev/null | closes | sort -un)
  fi
fi

[ $((${#nums[@]} + ${#carried[@]})) -gt 0 ] || exit 0

how_to_claim() { # <n>
  local hint=${claim_hint//<n>/$1}
  echo "  Claim it before working on it: scripts/issue.sh claim $1 (makes ../$(basename "$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")")-$1), or scripts/issue.sh claim $1 --here (for this worktree)${hint:+, or $hint}.
  Exit 2 from claim means another agent holds it: leave it and pick another issue (scripts/issue.sh list shows who holds what)."
}

denied=""
for n in $(printf '%s\n' ${nums[@]+"${nums[@]}"} | sort -un); do
  if [ -n "$push" ]; then why=$(bash "$tool" mine "$n" 2>&1)
  else why=$(bash "$tool" mine "$n" --offline 2>&1)
  fi
  [ $? -eq 0 ] && continue
  denied+="${why:-#$n is not claimed by this session}
$(how_to_claim "$n")
"
done

# A carried issue this session doesn't hold passes when GitHub says it's
# closed. One call asks about all of them, and only when one would be refused.
# Offline, none is known closed, and each is refused as before.
unheld=()
for n in ${carried[@]+"${carried[@]}"}; do
  bash "$tool" mine "$n" >/dev/null 2>&1 || unheld+=("$n")
done
if [ ${#unheld[@]} -gt 0 ]; then
  closed=$(bash "$tool" closed "${unheld[@]}" 2>/dev/null)
  for n in "${unheld[@]}"; do
    grep -qx "$n" <<<"$closed" && continue
    by=$(git log "${range[@]}" -i -E --grep="(close[sd]?|fix(e[sd])?|resolve[sd]?):?[[:space:]]+#$n([^0-9]|$)" \
      --format='%h "%s"' 2>/dev/null | head -1)
    denied+="#$n is named by a commit this push carries${by:+ ($by)}, and this session holds no claim on it.
  If that commit is a sibling's that this branch was built on, the push passes once #$n is closed on GitHub; it's open, or GitHub couldn't be asked.
$(how_to_claim "$n")
"
  done
fi
[ -z "$denied" ] && exit 0

{
  echo "Blocked by the claim rule: this ${commit:+commit}${commit:+${push:+ and }}${push:+push} is issue work this session hasn't claimed."
  printf '%s' "$denied"
  echo "Not issue work (a typo, a chore)? Keep it off issue-<n> branches and out of 'Closes #<n>' lines, and it needs no claim."
} >&2
exit 2
