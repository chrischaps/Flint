#!/usr/bin/env bash
# Build output across agent worktrees (#762). Every worktree builds its own
# target/ (CLAUDE.md, "Build output across worktrees" says why one shared
# CARGO_TARGET_DIR isn't safe here), so disk is what runs out. This keeps it
# in check:
#
#   scripts/build-space.sh check [--hook]   free space on this drive; exit 1
#                                           under the floor (2 with --hook: the
#                                           PreToolUse hook on issue.sh claim,
#                                           which refuses the claim)
#   scripts/build-space.sh reap [--dry-run] delete the target/ of every
#                                           worktree whose issue is closed
#   scripts/build-space.sh session          reap, then say if space is under
#                                           the floor (the SessionStart hook)
#   scripts/build-space.sh report           each worktree's target/ size and
#                                           issue state, biggest first (slow:
#                                           it measures every target/)
#
# A worktree's issue comes from its branch (issue-<n>, or a desktop-app
# worktree-<word>-<n>-<hex>) or its folder (Flint-<n>). Worktrees with no issue
# (main, feature branches) are never reaped; the sweep skill reviews those.
# reap deletes only target/, never the worktree or anything in git, skips this
# worktree and any target/ written to in the last hour (a build may be running),
# and stops starting new deletes after REAP_BUDGET seconds so a session start
# stays quick; the next session carries on.
#
# Environment:
#   FLINT_DISK_FLOOR_GB  check's floor (default 30: a cold worktree build plus
#                        its tests measured 6.6 GB, a release build adds
#                        more, and several sessions build at once)
#   REAP_BUDGET          seconds reap may spend deleting (default 40)
#   REAP_FRESH_MINUTES   a target/ written to more recently is kept (default 60)

set -uo pipefail

top=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "build-space.sh: not in a git checkout" >&2; exit 1; }
FLOOR=${FLINT_DISK_FLOOR_GB:-30}
BUDGET=${REAP_BUDGET:-40}
FRESH=${REAP_FRESH_MINUTES:-60}

free_gb() { df -Pk "$1" 2>/dev/null | awk 'NR == 2 { printf "%d", $4 / 1048576 }'; }

# worktrees: "<path>\t<issue number or ->" for every worktree of this repo.
worktrees() {
  local line path="" branch=""
  while IFS= read -r line; do
    case $line in
      "worktree "*) path=${line#worktree } branch="" ;;
      "branch refs/heads/"*) branch=${line#branch refs/heads/} ;;
      "") [ -z "$path" ] || { issue_of "$path" "$branch"; printf '%s\t%s\n' "$path" "$issue"; }; path="" ;;
    esac
  done < <(git -C "$top" worktree list --porcelain; echo)
}

# issue_of <path> <branch>: sets issue to the one a worktree works on, or -.
# No subshell: a fork costs tens of ms in Git Bash, and there are ~100 worktrees.
issue_of() {
  local folder=${1##*/}
  issue=-
  if [[ $2 =~ (^|/)issue-([0-9]+)$ ]]; then issue=${BASH_REMATCH[2]}
  elif [[ $2 =~ ^worktree-[a-z]+-([0-9]+)-[0-9a-f]+$ ]]; then issue=${BASH_REMATCH[1]}
  elif [[ $folder =~ ^[A-Za-z]+-([0-9]+)$ ]]; then issue=${BASH_REMATCH[1]}
  fi
}

cmd_check() {
  local gb
  gb=$(free_gb "$top")
  [ -n "$gb" ] || { echo "build-space.sh: couldn't read free space; not refusing"; return 0; }
  if [ "$gb" -lt "$FLOOR" ]; then
    cat >&2 <<EOF
build-space.sh: ${gb} GB free on this drive, under the ${FLOOR} GB floor. A new
worktree's cold build and tests take about 7 GB, more with a release build.
Free space first:
  bash scripts/build-space.sh reap      the target/ of closed issues' worktrees
  bash scripts/build-space.sh report    which worktrees hold the most
  the sweep skill                       stale worktrees and old cargo caches
Never delete another open issue's worktree or target/.
EOF
    return 1
  fi
  echo "${gb} GB free (floor ${FLOOR} GB)"
}

cmd_reap() {
  local dry="" here list nums closed path n start freed=0 skipped=()
  [ "${1:-}" = --dry-run ] && dry=1
  here=$(cd "$top" && pwd -W 2>/dev/null || pwd)
  list=$(worktrees)
  nums=$(awk -F'\t' '$2 != "-" { print $2 }' <<<"$list" | sort -un | tr '\n' ' ')
  [ -n "$nums" ] || return 0
  # shellcheck disable=SC2086
  closed=" $(bash "$top/scripts/issue.sh" closed $nums 2>/dev/null | tr '\n' ' ') "
  start=$(date +%s)
  while IFS=$'\t' read -r path n; do
    [ "$n" != "-" ] && [[ "$closed" == *" $n "* ]] || continue
    [ -d "$path/target" ] || continue
    [ "$path" != "$here" ] || continue
    # A build writing into deps/ (or incremental/) bumps the folder's mtime;
    # stat-ing the files themselves takes tens of seconds a target on NTFS.
    if [ -n "$(find "$path/target" "$path"/target/*/ "$path"/target/*/deps "$path"/target/*/incremental \
        "$path"/target/*/*/deps -maxdepth 0 -mmin "-$FRESH" -print -quit 2>/dev/null)" ]; then
      skipped+=("#$n (built in the last ${FRESH}m)"); continue
    fi
    if [ -n "$dry" ]; then
      echo "would delete $path/target (#$n is closed)"; continue
    fi
    if [ $(( $(date +%s) - start )) -ge "$BUDGET" ]; then
      skipped+=("#$n (out of time; next session)"); continue
    fi
    rm -rf "$path/target" 2>/dev/null && { freed=$((freed + 1)); echo "deleted $path/target (#$n is closed)"; } \
      || echo "couldn't fully delete $path/target (in use?)"
  done <<<"$list"
  [ ${#skipped[@]} -eq 0 ] || echo "kept for now: ${skipped[*]}"
  [ -n "$dry" ] || [ "$freed" -eq 0 ] || echo "freed $freed target dir(s); $(free_gb "$top") GB free"
}

cmd_report() {
  local list nums closed path n mb state total=0
  list=$(worktrees)
  nums=$(awk -F'\t' '$2 != "-" { print $2 }' <<<"$list" | sort -un | tr '\n' ' ')
  # shellcheck disable=SC2086
  closed=" $( [ -z "$nums" ] || bash "$top/scripts/issue.sh" closed $nums 2>/dev/null | tr '\n' ' ') "
  while IFS=$'\t' read -r path n; do
    [ -d "$path/target" ] || continue
    mb=$(du -sm "$path/target" 2>/dev/null | cut -f1)
    if [ "$n" = "-" ]; then state="no issue"
    elif [[ "$closed" == *" $n "* ]]; then state="#$n closed"
    else state="#$n open"; fi
    total=$((total + mb))
    printf '%s\t%s\t%s\n' "$mb" "$state" "$path"
  done <<<"$list" | sort -rn | awk -F'\t' '{ printf "%7.1f GB  %-12s %s\n", $1 / 1024, $2, $3; t += $1 } END { printf "%7.1f GB  total, %d target dirs\n", t / 1024, NR }'
  echo "$(free_gb "$top") GB free"
}

# session: what the SessionStart hook prints becomes the session's context, so
# it says nothing unless it freed something or space is short. Never fails.
cmd_session() {
  local out
  out=$(cmd_reap 2>&1)
  [ -z "$out" ] || printf 'Build output (scripts/build-space.sh):\n%s\n' "$(sed 's/^/  /' <<<"$out")"
  cmd_check >/dev/null 2>&1 || cmd_check 2>&1 | sed 's/^build-space.sh: /Disk: /'
  return 0
}

case ${1:-} in
  check) cmd_check || { [ "${2:-}" = --hook ] && exit 2; exit 1; } ;;
  reap) shift; cmd_reap "$@" ;;
  session) cmd_session ;;
  report) cmd_report ;;
  *) sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
