#!/usr/bin/env bash
# Claim, renew, release and land GitHub issues for agents that share one
# GitHub account across several machines.
#
# A claim is a branch on origin, claim/issue-<n>, holding one commit with no
# files whose message names the agent. Git updates a remote ref atomically,
# so when two agents push a claim for the same issue, the second push is
# rejected as non-fast-forward and only one agent wins. Labels, assignees and
# comments can't do that: every agent is the same GitHub user, and none of
# them is compare-and-swap.
#
#   scripts/issue.sh list                     every claim: issue, holder, age
#   scripts/issue.sh claim <n>                claim issue n (exit 2 if held) and
#                                             make its worktree, ../<repo>-<n>
#   scripts/issue.sh claim <n> --here         claim it for the worktree you're in
#                                             (one the desktop app made, say)
#   scripts/issue.sh claim <n> --steal        take over a claim older than the TTL
#   scripts/issue.sh claim <n> --steal --override "<reason>"
#                                             take over a younger claim, because
#                                             a human asked; the reason is recorded
#   scripts/issue.sh claim <n> --no-worktree  claim without making a worktree
#   scripts/issue.sh renew <n>                refresh your claim's timestamp
#   scripts/issue.sh release <n>              give up your claim
#   scripts/issue.sh land <n> [--verify CMD]  verify, then deliver: push mode
#                                             catches up with the base branch,
#                                             pushes to it and releases; pr mode
#                                             pushes the branch and opens a PR
#   scripts/issue.sh mine <n> [--offline]     exit 0 if you hold n here (hooks)
#   scripts/issue.sh closed <n>...            print those of n... closed on
#                                             GitHub, one a line (hooks)
#   scripts/issue.sh reap [--dry-run] [<n>...]
#                                             delete every claim (or n's) whose
#                                             issue is closed, whoever holds it
#   scripts/issue.sh merged <pr> [--dry-run]  reap the claims of the issues a
#                                             merged PR closes, and delete its
#                                             issue-<n> branch
#
# A pr-mode land leaves its claim on origin until the PR merges, and the
# merge closes the issue, so reap is where those claims end. The
# release-claim workflow (.github/workflows/release-claim.yml) runs merged
# when a PR merges and reap <n> when an issue closes, so a claim ends within
# a minute of its issue; the SessionStart hook reaps whatever that missed.
# Reaping is the one way to delete another agent's claim: a closed issue's
# claim guards nothing.
#
# --override is for a human who knows the holder is gone (a session they
# closed, a machine that's off). Agents use it only when asked to. The reason
# goes into the claim commit and the issue comment, so the takeover is on
# record.
#
# Two sessions started in one checkout share its .git, so the record of a
# claim can't by itself say which of them made it. So claim makes the
# worktree (or, with --here, takes the one it's run from), records its path
# beside the agent id, and renew, release and land run only from that
# worktree. A session whose claim exits 2 gets no worktree, and must leave
# the existing one alone.
#
# Per repo settings live in scripts/issue.conf, a bash file of plain
# assignments, all optional:
#   base=main                   branch work lands on (default: origin's HEAD)
#   land=pr                     push: fast-forward the base branch; pr: open a PR
#   verify="scripts/ci.sh"      land's --verify when none is given
#   accept_issue_branch=1       an issue-<n> branch on origin also counts as a
#                               claim, for repos that claimed that way before
#
# Environment (wins over issue.conf):
#   CLAIM_REMOTE     remote to use (default origin)
#   CLAIM_BASE, CLAIM_LAND, CLAIM_VERIFY   as base, land and verify above
#   CLAIM_TTL_HOURS  age after which a claim may be stolen (default 24)
#   CLAIM_AGENT_ID   name to claim under (default <host>-<time>-<random>);
#                    when set, renew, release and land refuse a claim made
#                    under another name
#   CLAIM_REAP_GRACE_MINUTES  how long reap spares the claim of a just-closed
#                    issue, in case it's reopened (default 10)
#   CLAIM_NO_GH=1    skip GitHub calls (labels, comments, issue state, PRs); for tests
#
# The canonical copy is skills/setup-claims in chrischaps/Dashboard; each repo
# vendors it. Change it there and re-run its install.sh, not here.

set -euo pipefail

CONF="$(git rev-parse --show-toplevel 2>/dev/null)/scripts/issue.conf"
base="" land="" verify="" accept_issue_branch=""
# shellcheck disable=SC1090
[ -f "$CONF" ] && . "$CONF"

REMOTE=${CLAIM_REMOTE:-origin}
TTL_HOURS=${CLAIM_TTL_HOURS:-24}
REAP_GRACE=${CLAIM_REAP_GRACE_MINUTES:-10}
LAND_MODE=${CLAIM_LAND:-${land:-push}}
DEFAULT_VERIFY=${CLAIM_VERIFY:-${verify:-}}
ISSUE_BRANCH_CLAIMS=${accept_issue_branch:-}
LABEL=claimed

die() { echo "issue.sh: $*" >&2; exit 1; }

usage() { sed -n '12,36p' "$0" | sed 's/^# \{0,1\}//'; exit 1; }

gh_on() { [ "${CLAIM_NO_GH:-}" != 1 ]; }

# The branch work lands on: issue.conf, else origin's HEAD, else main or master.
base_branch() {
  local b=${CLAIM_BASE:-$base}
  if [ -z "$b" ]; then
    b=$(git symbolic-ref -q --short "refs/remotes/$REMOTE/HEAD" 2>/dev/null || true)
    b=${b#"$REMOTE"/}
  fi
  if [ -z "$b" ] && git rev-parse -q --verify "$REMOTE/main" >/dev/null; then b=main; fi
  echo "${b:-master}"
}

ref_of() { echo "refs/heads/claim/issue-$1"; }

# Local record of the claims this checkout made, shared by all its worktrees.
# Line 1 is the agent id, line 2 the worktree the claim is for (absent after
# --no-worktree). Only the machine that claimed an issue can renew, release
# or land it, and only from that worktree.
common_dir() { git rev-parse --path-format=absolute --git-common-dir; }
record_dir() { echo "$(common_dir)/agent-claims"; }
record_of() { echo "$(record_dir)/issue-$1"; }
record_agent() { sed -n 1p "$(record_of "$1")"; }
record_worktree() { sed -n 2p "$(record_of "$1")"; }

# ../<repo>-<n> beside the main checkout, whichever worktree we're run from.
worktree_of() {
  local main
  main=$(dirname "$(common_dir)")
  echo "$(dirname "$main")/$(basename "$main")-$1"
}

now() { date +%s; }

# Prints "<sha> <committer time>" of the remote claim, or nothing if unclaimed.
remote_claim() {
  local ref sha
  ref=$(ref_of "$1")
  sha=$(git ls-remote "$REMOTE" "$ref" | cut -f1)
  [ -n "$sha" ] || return 0
  git fetch -q "$REMOTE" "$ref" 2>/dev/null || true
  echo "$sha $(git log -1 --format=%ct "$sha")"
}

# Every claim on origin, one "<n> <sha>" line each, in issue order.
remote_claims() {
  git ls-remote "$REMOTE" 'refs/heads/claim/issue-*' \
    | awk '{ n = $2; sub(/.*\/issue-/, "", n) } n ~ /^[0-9]+$/ { print n, $1 }' | sort -n
}

# Fetches the claim commits, so their messages and times can be read: in one
# fetch, or one at a time if a claim went away since it was listed.
fetch_claims() { # <n>...
  local n refs=()
  for n in "$@"; do refs+=("$(ref_of "$n")"); done
  [ ${#refs[@]} -gt 0 ] || return 0
  git fetch -q "$REMOTE" "${refs[@]}" 2>/dev/null && return 0
  for n in "$@"; do git fetch -q "$REMOTE" "$(ref_of "$n")" 2>/dev/null || true; done
}

# "<n> <STATE> <minutes since it closed, or ->" for each issue GitHub knows,
# in one GraphQL call. A number with no issue fails the whole batch, and so
# does a spent GraphQL budget, so then it asks one issue at a time over REST,
# whose budget is separate (#180). It reads both shapes: GraphQL's closedAt
# and OPEN/CLOSED/MERGED, REST's closed_at and open/closed.
JQ_STATE='"\(.number) \(.state | ascii_upcase) \((.closedAt // .closed_at) as $c | if $c then ((now - ($c | fromdateiso8601)) / 60 | floor) else "-" end)"'
issue_states() { # <n>...
  local n q="" out
  [ $# -gt 0 ] || return 0
  for n in "$@"; do
    q+=" i$n: issueOrPullRequest(number: $n) { ...on Issue { number state closedAt } ...on PullRequest { number state closedAt } }"
  done
  if out=$(gh api graphql -F owner='{owner}' -F repo='{repo}' \
      -f query="query(\$owner: String!, \$repo: String!) { repository(owner: \$owner, name: \$repo) {$q } }" \
      -q ".data.repository[] | select(. != null) | $JQ_STATE" 2>/dev/null); then
    echo "$out"
    return 0
  fi
  for n in "$@"; do gh api "repos/{owner}/{repo}/issues/$n" -q "$JQ_STATE" 2>/dev/null || true; done
}

# Everything else here talks to GitHub over REST. gh issue view/edit/comment
# and gh pr view/create spend the GraphQL budget, which an hour of busy agents
# can use up while REST's has barely moved; then land pushed the branch and
# couldn't open its PR (#180).
add_label() { # <n>: the claimed label, made the first time it's needed
  gh api "repos/{owner}/{repo}/labels" -f name="$LABEL" -f color=FBCA04 \
    -f description="an agent holds claim/issue-<n>" >/dev/null 2>&1 || true
  gh api "repos/{owner}/{repo}/issues/$1/labels" -f "labels[]=$LABEL" >/dev/null 2>&1 || true
}

drop_label() { # <n>...
  local n
  for n in "$@"; do gh api -X DELETE "repos/{owner}/{repo}/issues/$n/labels/$LABEL" >/dev/null 2>&1 || true; done
}

# The url of branch's open PR, or nothing.
pr_of_branch() { # <branch>
  gh api -X GET "repos/{owner}/{repo}/pulls" -F head="{owner}:$1" -f state=open -q '.[0].html_url // empty'
}

# Open a PR from branch to base with the title and body gh pr create --fill
# gives (gh's initDefaultTitleBody): one commit's subject and body; for
# several, the branch name with - and _ as spaces, and "- **subject**" for
# each commit, oldest first. Merges are left out, as gh's log --cherry does.
# Prints the PR's url.
open_pr() { # <base> <branch>
  local b=$1 branch=$2 range shas title body
  range="$REMOTE/$b..HEAD"
  shas=$(git rev-list --no-merges "$range")
  if [ "$(wc -l <<<"$shas")" = 1 ]; then
    title=$(git log -1 --format=%s "$shas")
    # gh keeps %b's trailing newline; $() would strip it without the x.
    body=$(git log -1 --format=%bx "$shas"); body=${body%x}
  else
    title=$(tr '_-' '  ' <<<"$branch")
    body=$(git log --no-merges --reverse --format='- **%s**' "$range")$'\n'
  fi
  gh api "repos/{owner}/{repo}/pulls" -f base="$b" -f head="$branch" -f title="$title" -f body="$body" -q .html_url
}

# The "<STATE> <minutes>" of n in issue_states' output (or the sha of n in
# remote_claims'), or nothing.
state_of() { awk -v n="$1" '$1 == n { $1 = ""; print substr($0, 2) }' <<<"$2"; }

is_closed() { [ "$1" = CLOSED ] || [ "$1" = MERGED ]; }

# The sha of origin's issue-<n> branch, when such branches count as claims.
issue_branch_claim() {
  [ "$ISSUE_BRANCH_CLAIMS" = 1 ] || return 0
  git ls-remote "$REMOTE" "refs/heads/issue-$1" | cut -f1
}

claim_body() { git log -1 --format=%B "$1"; }

agent_of() { claim_body "$1" | sed -n 's/^agent: //p'; }

age_text() {
  local s=$(( $(now) - $1 ))
  if [ "$s" -lt 3600 ]; then echo "$(( s / 60 ))m"; else echo "$(( s / 3600 ))h$(( s % 3600 / 60 ))m"; fi
}

make_claim() { # <n> <agent> [parent] [extra trailer lines]
  local msg parent=()
  [ -n "${3:-}" ] && parent=(-p "$3")
  msg="claim #$1

agent: $2
host: $(hostname)
at: $(date -u +%Y-%m-%dT%H:%M:%SZ)${4:+
$4}"
  printf '%s\n' "$msg" | git commit-tree "$(git mktree </dev/null)" ${parent[@]+"${parent[@]}"}
}

# Why this checkout doesn't hold n here, or nothing if it does. Local checks
# only: the record exists, the agent id matches, and we're in its worktree.
not_mine_locally() {
  local n=$1 rec agent wt here
  rec=$(record_of "$n")
  [ -f "$rec" ] || { echo "#$n wasn't claimed from this checkout (no $rec)"; return; }
  agent=$(record_agent "$n")
  if [ -n "${CLAIM_AGENT_ID:-}" ] && [ "$CLAIM_AGENT_ID" != "$agent" ]; then
    echo "#$n was claimed here as $agent, not as $CLAIM_AGENT_ID: it's another session's claim"
    return
  fi
  wt=$(record_worktree "$n")
  if [ -n "$wt" ]; then
    here=$(git rev-parse --show-toplevel)
    [ "$here" = "$wt" ] \
      || echo "#$n belongs to the session working in $wt; run this from there, not $here. If that session isn't you, leave it alone"
  fi
}

# Fails unless this checkout holds the claim and we're in the worktree that
# claim is for. Prints the remote claim sha.
require_mine() {
  local n=$1 why sha _t agent
  why=$(not_mine_locally "$n")
  [ -z "$why" ] || die "$why"
  agent=$(record_agent "$n")
  read -r sha _t <<<"$(remote_claim "$n")"
  [ -n "${sha:-}" ] || die "#$n has no claim on $REMOTE"
  [ "$(agent_of "$sha")" = "$agent" ] \
    || die "#$n is now held by $(agent_of "$sha"), not $agent"
  echo "$sha"
}

# mine <n> [--offline]: exit 0 if this session holds n here, else print why
# and exit 1. With accept_issue_branch, being on issue-<n> with origin/issue-<n>
# pushed counts too. --offline skips origin, so it's quick but only local.
cmd_mine() {
  local n=${1:?issue number} offline="" why sha _t
  [ "${2:-}" = --offline ] && offline=1
  why=$(not_mine_locally "$n")
  if [ -z "$why" ]; then
    [ -n "$offline" ] && return 0
    read -r sha _t <<<"$(remote_claim "$n")"
    if [ -z "${sha:-}" ]; then why="#$n has no claim on $REMOTE (released, or never pushed)"
    elif [ "$(agent_of "$sha")" != "$(record_agent "$n")" ]; then why="#$n is now held by $(agent_of "$sha")"
    else return 0
    fi
  fi
  if [ "$ISSUE_BRANCH_CLAIMS" = 1 ] && [ "$(git branch --show-current)" = "issue-$n" ]; then
    { [ -n "$offline" ] && git show-ref -q --verify "refs/remotes/$REMOTE/issue-$n"; } && return 0
    [ -n "$(issue_branch_claim "$n")" ] && return 0
  fi
  echo "$why"
  return 1
}

# closed <n>...: print the ones whose issue is closed on GitHub, one a line.
# It prints nothing when GitHub can't be asked (CLAIM_NO_GH=1, offline, no
# such issue), so a caller that skips what it prints errs toward refusing.
cmd_closed() {
  local n state _mins
  for n in "$@"; do
    case $n in ''|*[!0-9]*) die "closed: not an issue number: $n" ;; esac
  done
  gh_on || return 0
  while read -r n state _mins; do
    if is_closed "${state:-}"; then echo "$n"; fi
  done < <(issue_states "$@")
}

cmd_list() {
  local found=0 ref sha n claims states="" state mins note closed=()
  claims=$(remote_claims)
  if [ -n "$claims" ]; then
    found=1
    # shellcheck disable=SC2046
    fetch_claims $(cut -d' ' -f1 <<<"$claims")
    # shellcheck disable=SC2046
    ! gh_on || states=$(issue_states $(cut -d' ' -f1 <<<"$claims"))
    while read -r n sha; do
      note=""
      read -r state mins <<<"$(state_of "$n" "$states")"
      if is_closed "${state:-}"; then note="  issue closed"; closed+=("#$n"); fi
      printf '#%-5s %-40s %s ago%s\n' "$n" "$(agent_of "$sha")" "$(age_text "$(git log -1 --format=%ct "$sha")")" "$note"
    done <<<"$claims"
  fi
  if [ "$ISSUE_BRANCH_CLAIMS" = 1 ]; then
    while read -r sha ref; do
      [ -n "$sha" ] || continue
      found=1
      printf '#%-5s %-40s no TTL\n' "${ref##*/issue-}" "branch issue-${ref##*/issue-}"
    done < <(git ls-remote "$REMOTE" 'refs/heads/issue-*')
  fi
  [ "$found" = 1 ] || echo "no claims"
  [ ${#closed[@]} -eq 0 ] \
    || echo "${#closed[@]} of these are on closed issues (${closed[*]}); scripts/issue.sh reap releases them"
}

held() { # <n> <holder> <claim time or empty> <worktree>
  echo "#$1 is already claimed by $2${3:+, $(age_text "$3") ago}. It isn't yours: pick another issue, and don't touch $4" >&2
  exit 2
}

cmd_claim() {
  local n=${1:?issue number} steal="" override="" make_wt=1 here="" ref agent sha old old_t state wt base_ref="" note="" b
  shift
  while [ $# -gt 0 ]; do
    case $1 in
      --steal) steal=--steal ;;
      --override)
        [ $# -ge 2 ] && [ -n "${2// /}" ] && [ "${2#-}" = "$2" ] || die "--override needs a reason, e.g. --override \"Chris closed that session\""
        override=$2; shift ;;
      --no-worktree) make_wt="" ;;
      --here) make_wt="" here=1 ;;
      *) die "claim: unknown option $1" ;;
    esac
    shift
  done
  [ -z "$override" ] || [ -n "$steal" ] || die "--override goes with --steal"
  ref=$(ref_of "$n")
  if gh_on; then
    state=$(gh api "repos/{owner}/{repo}/issues/$n" -q 'if .pull_request then "a pull request" else (.state | ascii_upcase) end') \
      || die "couldn't read #$n from GitHub"
    [ "$state" = OPEN ] || die "#$n is $state"
  fi

  # A held claim is exit 2 even when its worktree is here, so the session
  # that lost knows to pick another issue.
  wt=$(worktree_of "$n")
  [ -z "$here" ] || wt=$(git rev-parse --show-toplevel)
  if [ -z "$steal" ]; then
    read -r old old_t <<<"$(remote_claim "$n")"
    [ -z "${old:-}" ] || held "$n" "$(agent_of "$old")" "$old_t" "$wt"
    if [ -n "$(issue_branch_claim "$n")" ] && [ "$(git branch --show-current)" != "issue-$n" ]; then
      held "$n" "the branch issue-$n on $REMOTE (an older-style claim)" "" "$wt"
    fi
  fi

  # Checked before claiming, so a refusal leaves nothing to undo. A worktree
  # or branch already here is another session's, or a stale one: never adopt it.
  if [ -n "$make_wt" ]; then
    [ ! -e "$wt" ] \
      || die "$wt already exists: another session's worktree, or a stale one. Leave it alone unless it's stale ('issue.sh list', 'git -C $wt status'); then push or drop its work, remove it, and claim again"
    ! git show-ref -q --verify "refs/heads/issue-$n" \
      || die "branch issue-$n already exists here. If it's stale, push or drop its work, delete it, and claim again"
  fi
  if [ -n "$here" ] && [ -f "$(record_of "$n")" ]; then
    # A record is written after its claim's push, so if origin has no claim
    # now, the record outlived one released or reaped elsewhere: drop it.
    [ -z "$steal" ] && [ -z "$(git ls-remote "$REMOTE" "$ref")" ] \
      || die "this checkout already has a record for #$n ($(record_of "$n")): another session's claim, or a stale one"
    echo "dropped this checkout's record of #$n: its claim is gone from $REMOTE"
    rm -f "$(record_of "$n")"
  fi

  agent=${CLAIM_AGENT_ID:-$(hostname)-$(now)-$RANDOM}

  if [ -n "$steal" ]; then
    read -r old old_t <<<"$(remote_claim "$n")"
    [ -n "${old:-}" ] || die "#$n isn't claimed; claim it without --steal"
    if [ -n "$override" ]; then
      note="taken over from \`$(agent_of "$old")\`'s $(age_text "$old_t")-old claim, by override: $override"
      sha=$(make_claim "$n" "$agent" "" "stolen-from: $(agent_of "$old")
override: $override")
    else
      [ $(( $(now) - old_t )) -ge $(( TTL_HOURS * 3600 )) ] \
        || die "#$n claim by $(agent_of "$old") is $(age_text "$old_t") old, under the ${TTL_HOURS}h TTL. Only a human can say it's gone sooner: then add --override \"<reason>\""
      note="taken over from \`$(agent_of "$old")\`'s stale claim"
      sha=$(make_claim "$n" "$agent" "" "stolen-from: $(agent_of "$old")")
    fi
    # Compare-and-swap: fails if the holder renewed since we looked.
    git push -q --force-with-lease="$ref:$old" "$REMOTE" "$sha:$ref" \
      || die "#$n claim changed while stealing it; run list and look again"
  else
    sha=$(make_claim "$n" "$agent")
    if ! git push -q "$REMOTE" "$sha:$ref" 2>/dev/null; then
      read -r old old_t <<<"$(remote_claim "$n")"
      [ -n "${old:-}" ] || die "push of the claim for #$n failed, and nobody holds it; check the network and retry"
      held "$n" "$(agent_of "$old")" "$old_t" "$wt"
    fi
  fi

  mkdir -p "$(record_dir)"
  if [ -n "$make_wt" ]; then
    # Continue an earlier agent's pushed work in progress, if there is any.
    b=$(base_branch)
    git fetch -q "$REMOTE" "$b"
    base_ref=$REMOTE/$b
    if git fetch -q "$REMOTE" "refs/heads/issue-$n:refs/remotes/$REMOTE/issue-$n" 2>/dev/null; then
      base_ref=$REMOTE/issue-$n
    fi
    if ! git worktree add -q "$wt" -b "issue-$n" "$base_ref"; then
      git push -q --force-with-lease="$ref:$sha" "$REMOTE" ":$ref" || true
      die "couldn't make the worktree $wt, so the claim on #$n was dropped"
    fi
    wt=$(git -C "$wt" rev-parse --show-toplevel)
    printf '%s\n%s\n' "$agent" "$wt" >"$(record_of "$n")"
  elif [ -n "$here" ]; then
    printf '%s\n%s\n' "$agent" "$wt" >"$(record_of "$n")"
  else
    echo "$agent" >"$(record_of "$n")"
  fi
  if gh_on; then
    add_label "$n"
    gh api "repos/{owner}/{repo}/issues/$n/comments" \
      -f body="Claimed by \`$agent\` on \`$(hostname)\`${note:+, $note}." >/dev/null || true
  fi
  echo "claimed #$n as $agent${note:+, ${note//\`/}}"
  if [ -n "$make_wt" ]; then
    echo "worktree: $wt (branch issue-$n from $base_ref). Work there; renew, release and land run only from it"
  elif [ -n "$here" ]; then
    echo "for this worktree: $wt. Renew, release and land run only from it"
  fi
}

cmd_renew() {
  local n=${1:?issue number} old new
  old=$(require_mine "$n")
  new=$(make_claim "$n" "$(record_agent "$n")" "$old")
  git push -q "$REMOTE" "$new:$(ref_of "$n")" || die "renew of #$n was rejected"
  echo "renewed #$n"
}

cmd_release() {
  local n=${1:?issue number} sha
  sha=$(require_mine "$n")
  git push -q --force-with-lease="$(ref_of "$n"):$sha" "$REMOTE" ":$(ref_of "$n")" \
    || die "release of #$n was rejected"
  rm -f "$(record_of "$n")"
  ! gh_on || drop_label "$n"
  echo "released #$n"
}

# reap [--dry-run] [<n>...]: delete the claims whose issue is closed (only
# n's, when given), whoever holds them, drop the claimed label, and drop this
# checkout's records of claims that are gone from origin. Each delete is
# leased on the sha just listed, so a claim renewed or stolen meanwhile
# stays. A claim on an open issue, or on a number GitHub doesn't answer for,
# is never touched.
cmd_reap() {
  local dry="" only="" claims states n sha state mins specs=() leases=() reaped=() kept=() moved=() gone=() flag spec _rest recs=() rec
  while [ $# -gt 0 ]; do
    case $1 in
      --dry-run) dry=1 ;;
      ''|*[!0-9]*) die "reap: unknown option $1" ;;
      *) only+=" $1" ;;
    esac
    shift
  done
  gh_on || die "reap reads issue states from GitHub, so it can't run with CLAIM_NO_GH=1"
  claims=$(remote_claims)
  [ -z "$only" ] || claims=$(awk -v only="$only " 'index(only, " " $1 " ")' <<<"$claims")
  if [ -n "$claims" ]; then
    # shellcheck disable=SC2046
    fetch_claims $(cut -d' ' -f1 <<<"$claims")
    # shellcheck disable=SC2046
    states=$(issue_states $(cut -d' ' -f1 <<<"$claims"))
    while read -r n sha; do
      read -r state mins <<<"$(state_of "$n" "$states")"
      is_closed "${state:-}" || continue
      case $mins in ''|*[!0-9]*) continue ;; esac
      if [ "$mins" -lt "$REAP_GRACE" ]; then
        echo "kept #$n: closed ${mins}m ago, under the ${REAP_GRACE}m grace in case it's reopened"
        continue
      fi
      if [ -n "$dry" ]; then
        echo "would reap #$n: $(agent_of "$sha")'s claim, issue closed $(age_text $(( $(now) - mins * 60 ))) ago"
      else
        specs+=(":$(ref_of "$n")")
        leases+=("--force-with-lease=$(ref_of "$n"):$sha")
      fi
    done <<<"$claims"
  fi
  if [ ${#specs[@]} -gt 0 ]; then
    # One push for every delete; --porcelain says which of them went through.
    while IFS=$'\t' read -r flag spec _rest; do
      case $flag in
        -) reaped+=("${spec##*/issue-}") ;;
        '!') kept+=("${spec##*/issue-}") ;;
      esac
    done < <(git push --porcelain "${leases[@]}" "$REMOTE" "${specs[@]}" 2>/dev/null || true)
    if [ ${#kept[@]} -gt 0 ]; then
      # A refused delete is a claim that moved, or one already gone: the
      # workflow's run for the merge and its run for the issue race each other.
      claims=$(remote_claims)
      for n in "${kept[@]}"; do
        if [ -n "$(state_of "$n" "$claims")" ]; then moved+=("$n"); else gone+=("$n"); fi
      done
      kept=(${moved[@]+"${moved[@]}"})
      [ ${#gone[@]} -eq 0 ] || echo "already released: $(printf '#%s ' "${gone[@]}")"
    fi
    [ ${#kept[@]} -eq 0 ] || echo "kept $(printf '#%s ' "${kept[@]}")(changed since listed: renewed or stolen)"
    if [ ${#reaped[@]} -gt 0 ]; then
      drop_label "${reaped[@]}"
      for n in "${reaped[@]}"; do rm -f "$(record_of "$n")"; done
      echo "reaped ${#reaped[@]} claim(s) on closed issues: $(printf '#%s ' "${reaped[@]}")"
    fi
  fi
  # A record outliving its claim is inert (every use checks origin too), but
  # it blocks claim --here and clutters the session start. Records are listed
  # before origin is, and each is written after its claim's push, so a claim
  # made meanwhile is never taken for a gone one.
  for rec in "$(record_dir)"/issue-*; do
    [ -f "$rec" ] || continue
    n=${rec##*/issue-}
    [ -z "$only" ] || [[ "$only " == *" $n "* ]] || continue
    recs+=("$n")
  done
  [ ${#recs[@]} -gt 0 ] || return 0
  claims=$(remote_claims)
  for n in "${recs[@]}"; do
    [ -z "$(state_of "$n" "$claims")" ] || continue
    if [ -n "$dry" ]; then
      echo "would drop this checkout's record of #$n: its claim is gone from $REMOTE"
    else
      rm -f "$(record_of "$n")"
      echo "dropped this checkout's record of #$n: its claim is gone from $REMOTE"
    fi
  done
}

# The issues a PR's text closes, one number per line: its closing keywords as
# land checks for them, digits only, never the branch name.
closing_numbers() {
  grep -oiE '\b(close[sd]?|fix(e[sd])?|resolve[sd]?):? +#[0-9]+\b' | grep -oE '[0-9]+$' | sort -un || true
}

# Line 1 "<state> <cross-repo> <head branch> <head sha>"; then every text
# that can close an issue: GitHub's own list of the issues the PR closes (its
# body, and any linked by hand), the body, and each commit's message.
JQ_PR='"\(.state) \(.isCrossRepository) \(.headRefName) \(.headRefOid)", (.closingIssuesReferences[] | "closes #\(.number)"), .body, (.commits[] | .messageHeadline, .messageBody)'

# merged <pr> [--dry-run]: what the release-claim workflow runs when a PR
# merges. Reaps the claims of the issues it closes, once GitHub has closed
# them (the workflow's run for each issue's close catches any it hasn't
# yet), and deletes its head branch if that is issue-<n> for one of them,
# leased on the merged sha so work pushed after the merge stays.
cmd_merged() {
  local pr=${1:?PR number} dry="" info state cross branch sha nums n
  [ "${2:-}" = --dry-run ] && dry=--dry-run
  case $pr in *[!0-9]*) die "merged: $pr isn't a PR number" ;; esac
  gh_on || die "merged reads the PR from GitHub, so it can't run with CLAIM_NO_GH=1"
  info=$(gh pr view "$pr" --json state,isCrossRepository,headRefName,headRefOid,closingIssuesReferences,body,commits \
    -q "$JQ_PR") || die "couldn't read PR #$pr"
  read -r state cross branch sha <<<"$(head -n 1 <<<"$info")"
  if [ "$state" != MERGED ]; then
    echo "PR #$pr is ${state:-unknown}, not merged: every claim stays"
    return 0
  fi
  nums=$(tail -n +2 <<<"$info" | closing_numbers)
  if [ -z "$nums" ]; then
    echo "PR #$pr closes no issue: every claim stays"
    return 0
  fi
  echo "PR #$pr closes $(printf '#%s ' $nums)"
  # shellcheck disable=SC2086
  cmd_reap $dry $nums
  [ "$cross" = false ] && [[ $branch =~ ^issue-([0-9]+)$ ]] || return 0
  n=${BASH_REMATCH[1]}
  grep -qx "$n" <<<"$nums" || return 0
  if [ -n "$dry" ]; then
    echo "would delete $branch, merged as PR #$pr"
  elif git push -q --force-with-lease="refs/heads/$branch:$sha" "$REMOTE" ":refs/heads/$branch" 2>/dev/null; then
    echo "deleted $branch, merged as PR #$pr"
  else
    echo "kept $branch: gone already, or it moved since the merge"
  fi
}

run_verify() { echo "verifying: $1"; bash -c "$1" || die "verify failed; fix it, commit, land again"; }

cmd_land() {
  local n=${1:?issue number} verify="" attempt b branch url
  shift
  if [ "${1:-}" = --verify ]; then verify=${2:?--verify needs a command}; else verify=$DEFAULT_VERIFY; fi
  require_mine "$n" >/dev/null
  git diff --quiet && git diff --cached --quiet || die "commit or drop your changes first"
  b=$(base_branch)
  git fetch -q "$REMOTE" "$b"
  git log "$REMOTE/$b..HEAD" --format=%B | grep -qiE "(close[sd]?|fix(e[sd])?|resolve[sd]?) #$n\b" \
    || echo "issue.sh: warning: no commit says 'Closes #$n'" >&2

  if [ "$LAND_MODE" = pr ]; then
    # The branch goes up for review; the claim stays until the PR merges and
    # closes the issue (the release-claim workflow reaps it then), so nobody
    # else picks the issue up meanwhile.
    branch=$(git branch --show-current)
    [ -n "$branch" ] && [ "$branch" != "$b" ] || die "land in pr mode needs a work branch, not $b"
    [ -z "$verify" ] || run_verify "$verify"
    git push -q -u "$REMOTE" "HEAD:refs/heads/$branch" || die "push of $branch was rejected; fetch, catch up with $REMOTE/$branch, land again"
    if gh_on; then
      url=$(pr_of_branch "$branch" 2>/dev/null || true)
      if [ -n "$url" ]; then
        echo "pushed $branch; its PR: $url"
      else
        url=$(open_pr "$b" "$branch") && [ -n "$url" ] || die "pushed $branch, but couldn't open a PR; open it by hand"
        echo "opened $url"
      fi
    else
      echo "pushed $branch"
    fi
    echo "the claim on #$n ends when the PR merges and closes the issue: the release-claim workflow releases it then, so there's nothing to run (release is for giving up)"
    return 0
  fi

  # The commit --verify last passed on. Verify runs before the first push and
  # again whenever catching up with the base branch changes HEAD, so a land
  # after a conflict resolved by hand is still verified.
  local verified=""
  for attempt in 1 2 3 4 5; do
    if ! git merge-base --is-ancestor "$REMOTE/$b" HEAD; then
      # A rebase flattens merges, and a subtree pull's merge is what the next
      # `git subtree pull`/`push` finds its base by. So a branch holding any
      # merge, or a squashed subtree commit, merges the base branch in instead.
      if [ -n "$(git rev-list --merges "$REMOTE/$b..HEAD")" ] \
        || git log "$REMOTE/$b..HEAD" --format=%B | grep -q '^git-subtree-dir:'; then
        echo "$b moved; merging it in to keep the branch's merges (attempt $attempt)"
        git merge -q --no-edit "$REMOTE/$b" || {
          echo "issue.sh: merge conflict. Resolve it, 'git commit', then land again." >&2
          exit 3
        }
      else
        echo "$b moved; rebasing (attempt $attempt)"
        git rebase -q "$REMOTE/$b" || {
          echo "issue.sh: rebase conflict. Resolve it, 'git rebase --continue', then land again." >&2
          exit 3
        }
      fi
    fi
    if [ -n "$verify" ] && [ "$verified" != "$(git rev-parse HEAD)" ]; then
      run_verify "$verify"
      verified=$(git rev-parse HEAD)
    fi
    # Fast-forward only: a rejection means someone landed first, so loop.
    if git push -q "$REMOTE" "HEAD:$b"; then
      echo "landed #$n at $(git rev-parse --short HEAD)"
      cmd_release "$n"
      git push -q "$REMOTE" --delete "issue-$n" 2>/dev/null || true
      return 0
    fi
    git fetch -q "$REMOTE" "$b"
  done
  die "$b kept moving; land again"
}

case ${1:-} in
  list) cmd_list ;;
  claim) shift; cmd_claim "$@" ;;
  renew) shift; cmd_renew "$@" ;;
  release) shift; cmd_release "$@" ;;
  land) shift; cmd_land "$@" ;;
  mine) shift; cmd_mine "$@" ;;
  closed) shift; cmd_closed "$@" ;;
  reap) shift; cmd_reap "$@" ;;
  merged) shift; cmd_merged "$@" ;;
  *) usage ;;
esac
