#!/usr/bin/env bash
# Tests for scripts/issue.sh and scripts/issue-guard.sh against a throwaway bare remote, with no GitHub
# calls. Run: bash scripts/issue-test.sh
set -uo pipefail

SCRIPT=$(cd "$(dirname "$0")" && pwd)/issue.sh
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
export CLAIM_NO_GH=1 GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
unset CLAIM_AGENT_ID

fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fails=$((fails + 1)); }
# expect <code> <name> <cmd...>: run cmd, check its exit code
expect() {
  local want=$1 name=$2 got; shift 2
  "$@" >"$TMP/out" 2>&1; got=$?
  if [ "$got" = "$want" ]; then ok "$name"; else bad "$name (exit $got, wanted $want)"; sed 's/^/     /' "$TMP/out"; fi
}

git init -q --bare -b master "$TMP/remote.git"
git clone -q "$TMP/remote.git" "$TMP/repo" 2>/dev/null
cd "$TMP/repo"
git commit -q --allow-empty -m init && git push -q origin master
REPO=$TMP/repo
WT=$TMP/repo-7

expect 0 "claim makes the worktree" bash "$SCRIPT" claim 7
[ -d "$WT" ] && [ "$(git -C "$WT" branch --show-current)" = issue-7 ] \
  && ok "worktree is on issue-7" || bad "worktree is on issue-7"

# A second session in the same checkout: its claim loses, and it can't act
# on the first session's claim from the main checkout.
expect 2 "second claim exits 2" bash "$SCRIPT" claim 7
grep -q "don't touch" "$TMP/out" && ok "exit 2 names the worktree to leave alone" || bad "exit 2 names the worktree"
expect 1 "renew from the main checkout is refused" bash "$SCRIPT" renew 7
expect 1 "release from the main checkout is refused" bash "$SCRIPT" release 7
expect 1 "land from the main checkout is refused" bash "$SCRIPT" land 7

cd "$WT"
expect 0 "renew from the worktree" bash "$SCRIPT" renew 7
expect 1 "a different CLAIM_AGENT_ID is refused" env CLAIM_AGENT_ID=someone-else bash "$SCRIPT" renew 7
agent=$(sed -n 1p "$REPO/.git/agent-claims/issue-7")
expect 0 "the claiming CLAIM_AGENT_ID is accepted" env CLAIM_AGENT_ID="$agent" bash "$SCRIPT" renew 7

echo x >f && git add f && git commit -q -m "do it

Closes #7"
expect 0 "land from the worktree" bash "$SCRIPT" land 7 --verify true
[ "$(git ls-remote origin refs/heads/claim/issue-7)" = "" ] && ok "land released the claim" || bad "land released the claim"
[ "$(git rev-parse origin/master)" = "$(git rev-parse HEAD)" ] && ok "master moved to the work" || bad "master moved"

cd "$REPO"
git merge -q --ff-only origin/master
git worktree remove "$WT" && git branch -q -D issue-7

# A leftover worktree or branch stops a claim before it's pushed.
mkdir "$TMP/repo-8"
expect 1 "claim refuses an existing worktree path" bash "$SCRIPT" claim 8
[ "$(git ls-remote origin refs/heads/claim/issue-8)" = "" ] && ok "and claims nothing" || bad "and claims nothing"
rmdir "$TMP/repo-8"
git branch -q issue-8
expect 1 "claim refuses an existing issue branch" bash "$SCRIPT" claim 8
git branch -q -D issue-8

# Pushed work in progress is where a new claim's worktree starts.
git push -q origin master:refs/heads/issue-9
git commit -q --allow-empty -m "master moves on" && git push -q origin master
expect 0 "claim of an issue with work in progress" bash "$SCRIPT" claim 9
[ "$(git -C "$TMP/repo-9" rev-parse HEAD)" = "$(git rev-parse origin/issue-9)" ] \
  && ok "worktree starts from origin/issue-9" || bad "worktree starts from origin/issue-9"
(cd "$TMP/repo-9" && bash "$SCRIPT" release 9 >/dev/null) && ok "release from the worktree" || bad "release from the worktree"

# --no-worktree keeps the old behaviour: no worktree, act from anywhere.
expect 0 "claim --no-worktree" bash "$SCRIPT" claim 10 --no-worktree
[ ! -e "$TMP/repo-10" ] && ok "makes no worktree" || bad "makes no worktree"
expect 0 "release a --no-worktree claim from the main checkout" bash "$SCRIPT" release 10

# A young claim can't be stolen, except by an override with a reason (#140).
CLAIM_AGENT_ID=holder bash "$SCRIPT" claim 11 --no-worktree >/dev/null
expect 1 "steal of a young claim is refused" bash "$SCRIPT" claim 11 --steal --no-worktree
grep -q -- "--override" "$TMP/out" && ok "the refusal points at --override" || bad "the refusal points at --override"
expect 1 "--override without a reason is refused" bash "$SCRIPT" claim 11 --steal --override --no-worktree
expect 1 "--override with a blank reason is refused" bash "$SCRIPT" claim 11 --steal --override "  " --no-worktree
expect 1 "--override without --steal is refused" bash "$SCRIPT" claim 11 --override "gone" --no-worktree
expect 0 "--steal --override takes a young claim" \
  env CLAIM_AGENT_ID=taker bash "$SCRIPT" claim 11 --steal --override "Chris closed that session" --no-worktree
grep -q "by override: Chris closed that session" "$TMP/out" && ok "the output gives the reason" || bad "the output gives the reason"
body=$(git log -1 --format=%B "$(git ls-remote origin refs/heads/claim/issue-11 | cut -f1)")
grep -q "^agent: taker$" <<<"$body" && grep -q "^stolen-from: holder$" <<<"$body" \
  && grep -q "^override: Chris closed that session$" <<<"$body" \
  && ok "the claim commit records holder and reason" || bad "the claim commit records holder and reason"
expect 0 "the new holder can release it" env CLAIM_AGENT_ID=taker bash "$SCRIPT" release 11

# --here claims for the worktree it's run from (one the desktop app made).
git worktree add -q "$TMP/app-wt" -b claude/some-name master
cd "$TMP/app-wt"
expect 0 "claim --here" bash "$SCRIPT" claim 12 --here
[ "$(sed -n 2p "$REPO/.git/agent-claims/issue-12")" = "$(git rev-parse --show-toplevel)" ] \
  && ok "the record names this worktree" || bad "the record names this worktree"
expect 0 "mine from that worktree" bash "$SCRIPT" mine 12
expect 0 "mine --offline from that worktree" bash "$SCRIPT" mine 12 --offline
cd "$REPO"
expect 1 "mine from the main checkout is refused" bash "$SCRIPT" mine 12 --offline
expect 1 "mine of an unclaimed issue is refused" bash "$SCRIPT" mine 13 --offline

# The guard: a commit or push of issue work this session hasn't claimed is denied.
GUARD=$(dirname "$SCRIPT")/issue-guard.sh
mkdir -p "$REPO/scripts" "$TMP/app-wt/scripts"
cp "$SCRIPT" "$REPO/scripts/issue.sh"; cp "$SCRIPT" "$TMP/app-wt/scripts/issue.sh"
hook() { # <cwd> <command>: the JSON Claude Code sends a PreToolUse hook
  local c=${2//\\/\\\\}; c=${c//\"/\\\"}; c=${c//$'\n'/\\n}
  printf '{"session_id":"s","hook_event_name":"PreToolUse","tool_name":"Bash","cwd":"%s","tool_input":{"command":"%s"}}' "$1" "$c" | bash "$GUARD"
}
expect 0 "guard: an unrelated command passes" hook "$REPO" "ls -la"
expect 0 "guard: a commit with no issue passes" hook "$REPO" 'git commit -m "fix a typo"'
expect 2 "guard: Closes #13, unclaimed, is denied" hook "$REPO" 'git commit -m "$(cat <<'"'"'EOF'"'"'
do it

Closes #13
EOF
)"'
grep -q "scripts/issue.sh claim 13" "$TMP/out" && ok "the denial says how to claim" || bad "the denial says how to claim"
expect 0 "guard: Closes #12 from its worktree passes" hook "$TMP/app-wt" 'git commit -m "x

Fixes #12"'
expect 2 "guard: Closes #12 from the main checkout is denied" hook "$REPO" 'git commit -m "Closes #12"'
expect 0 "guard: cd into the claiming worktree first passes" hook "$REPO" "cd \"$TMP/app-wt\" && git commit -m 'Closes #12'"
git worktree add -q "$TMP/repo-14" -b issue-14 master
mkdir -p "$TMP/repo-14/scripts" && cp "$SCRIPT" "$TMP/repo-14/scripts/issue.sh"
expect 2 "guard: a commit on an unclaimed issue-14 branch is denied" hook "$TMP/repo-14" 'git commit -m "wip"'
(cd "$TMP/app-wt" && echo y >g && git add g && git commit -q -m "Closes #12")
expect 0 "guard: push of claimed work passes" hook "$TMP/app-wt" "git push -u origin HEAD"
expect 2 "guard: push from a branch with someone else's Closes #13 is denied" \
  bash -c "cd '$TMP/app-wt' && git commit -q --allow-empty -m 'Closes #13' && $(declare -f hook); GUARD='$GUARD' hook '$TMP/app-wt' 'git push'"
(cd "$TMP/app-wt" && git reset -q --hard HEAD~1)
expect 0 "guard: deleting a branch passes" hook "$REPO" "git push origin --delete issue-14"
git worktree remove -f "$TMP/repo-14"; git branch -q -D issue-14

# Once released, the worktree's push is denied: the claim is gone from origin.
(cd "$TMP/app-wt" && bash "$SCRIPT" release 12 >/dev/null)
expect 2 "guard: push after release is denied" hook "$TMP/app-wt" "git push"

# pr mode: land pushes the branch and keeps the claim.
cd "$TMP/app-wt"
bash "$SCRIPT" claim 15 --here >/dev/null
git commit -q --allow-empty -m "Closes #15"
expect 0 "land in pr mode" env CLAIM_LAND=pr bash "$SCRIPT" land 15 --verify true
[ -n "$(git ls-remote origin refs/heads/claude/some-name)" ] && ok "pr mode pushed the branch" || bad "pr mode pushed the branch"
[ -n "$(git ls-remote origin refs/heads/claim/issue-15)" ] && ok "pr mode keeps the claim" || bad "pr mode keeps the claim"
[ "$(git rev-parse origin/master)" != "$(git rev-parse HEAD)" ] && ok "pr mode leaves master alone" || bad "pr mode leaves master alone"
bash "$SCRIPT" release 15 >/dev/null
cd "$REPO"

# reap: a closed issue's claim is deleted, whoever holds it; one on an open
# issue, a just-closed one or a number GitHub doesn't know stays (#175). A
# stub gh answers issue states from $GH_STATES ("<n> <STATE> <minutes since
# closed>"), runs $GH_MEANWHILE when asked, and logs every call. With
# GH_NO_GRAPHQL set it fails every GraphQL call (gh api graphql, gh pr, gh
# issue) the way a spent budget does, and answers the REST ones (#180): an
# issue's state from $GH_STATES, the open PR from $GH_PULLS, and a new PR by
# keeping each field in $GH_FIELDS/<name>.
mkdir -p "$TMP/bin"
cat >"$TMP/bin/gh" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$GH_LOG"
if [ -n "${GH_NO_GRAPHQL:-}" ] && { [ "$1 $2" = "api graphql" ] || [ "$1" = pr ] || [ "$1" = issue ]; }; then
  echo "GraphQL: API rate limit already exceeded for user ID 1." >&2
  exit 1
fi
if [ "$1 $2" = "api graphql" ]; then
  [ -z "${GH_MEANWHILE:-}" ] || bash -c "$GH_MEANWHILE"
  cat "$GH_STATES"
fi
[ "$1 $2" != "pr view" ] || cat "$GH_PR"
case "$*" in
  "api repos/{owner}/{repo}/issues/"[0-9]*" -q "*)
    n=${2##*/}; awk -v n="$n" '$1 == n' "$GH_STATES" | grep . ;;
  "api -X GET repos/{owner}/{repo}/pulls "*) cat "$GH_PULLS" ;;
  "api repos/{owner}/{repo}/pulls "*)
    while [ $# -gt 0 ]; do [ "$1" != -f ] || printf '%s' "${2#*=}" >"$GH_FIELDS/${2%%=*}"; shift; done
    echo "https://github.com/o/r/pull/99" ;;
esac
EOF
chmod +x "$TMP/bin/gh"
printf '%s\n' "20 CLOSED 600" "21 OPEN -" "22 CLOSED 3" "23 CLOSED 600" >"$TMP/states"
export GH_LOG=$TMP/gh.log GH_STATES=$TMP/states GH_PULLS=$TMP/pulls GH_FIELDS=$TMP/fields
withgh() { env CLAIM_NO_GH= PATH="$TMP/bin:$PATH" "$@"; }
unlabeled() { grep -qx "api -X DELETE repos/{owner}/{repo}/issues/$1/labels/claimed" "$GH_LOG"; }
claim_of() { git ls-remote origin "refs/heads/claim/issue-$1" | cut -f1; }
for n in 20 21 22 23 24 25; do CLAIM_AGENT_ID=elsewhere-$n bash "$SCRIPT" claim $n --no-worktree >/dev/null; done
git push -q origin --delete claim/issue-24 # released elsewhere; the record here outlives it
RECS=$REPO/.git/agent-claims

expect 1 "reap refuses to run without GitHub" bash "$SCRIPT" reap
: >"$GH_LOG"
expect 0 "reap --dry-run" withgh bash "$SCRIPT" reap --dry-run
grep -q "would reap #20: elsewhere-20's claim" "$TMP/out" && grep -q "would reap #23" "$TMP/out" \
  && ok "the dry run lists the closed issues' claims" || bad "the dry run lists the closed issues' claims"
! grep -qE "would reap #(21|22|25)" "$TMP/out" \
  && ok "and none on an open, just-closed or unknown issue" || bad "and none on an open, just-closed or unknown issue"
grep -q "would drop this checkout's record of #24" "$TMP/out" \
  && ok "and the record whose claim is gone" || bad "and the record whose claim is gone"
[ -n "$(claim_of 20)" ] && [ -n "$(claim_of 23)" ] && [ -f "$RECS/issue-20" ] && [ -f "$RECS/issue-24" ] \
  && ! grep -q "labels/claimed" "$GH_LOG" && ok "the dry run changes nothing" || bad "the dry run changes nothing"

# #23 is renewed on another machine after reap lists it: its leased delete fails.
new23=$(git commit-tree -p "$(claim_of 23)" -m renewed "$(git mktree </dev/null)")
expect 0 "reap" withgh env GH_MEANWHILE="git -C '$REPO' push -q origin $new23:refs/heads/claim/issue-23" bash "$SCRIPT" reap
[ -z "$(claim_of 20)" ] && ok "reap deleted the closed issue's claim" || bad "reap deleted the closed issue's claim"
[ -n "$(claim_of 21)" ] && [ -n "$(claim_of 22)" ] && [ -n "$(claim_of 25)" ] \
  && ok "and spared the open, just-closed and unknown issues' claims" || bad "and spared the open, just-closed and unknown issues' claims"
[ "$(claim_of 23)" = "$new23" ] && grep -q "kept #23" "$TMP/out" \
  && ok "a claim renewed since it was listed is kept" || bad "a claim renewed since it was listed is kept"
unlabeled 20 && ! grep -q "issues/2[1-5]/labels" "$GH_LOG" \
  && ok "the claimed label comes off the reaped issue only" || bad "the claimed label comes off the reaped issue only"
[ ! -f "$RECS/issue-20" ] && [ ! -f "$RECS/issue-24" ] && [ -f "$RECS/issue-21" ] \
  && ok "the records of gone claims are dropped, the rest kept" || bad "the records of gone claims are dropped, the rest kept"
withgh bash "$SCRIPT" list >"$TMP/out"
grep -q "^#22 .*issue closed$" "$TMP/out" && ! grep -q "^#21 .*issue closed" "$TMP/out" \
  && grep -q "on closed issues (#22 #23); scripts/issue.sh reap releases them" "$TMP/out" \
  && ok "list marks the claims on closed issues and offers reap" || bad "list marks the claims on closed issues and offers reap"
for n in 21 22 23 25; do git push -q origin --delete "claim/issue-$n"; rm -f "$RECS/issue-$n"; done

# reap <n>... and merged <pr>: what the release-claim workflow runs when an
# issue closes and when a PR merges (#176), with no grace. The stub answers
# `gh pr view` from $GH_PR, already filtered: "<state> <cross-repo> <head
# branch> <head sha>", then the texts that can close an issue.
export CLAIM_REAP_GRACE_MINUTES=0 GH_PR=$TMP/pr
printf '%s\n' "30 CLOSED 0" "31 OPEN -" "32 CLOSED 0" "33 CLOSED 0" "36 CLOSED 0" >"$GH_STATES"
for n in 30 31 32 33 36; do CLAIM_AGENT_ID=elsewhere-$n bash "$SCRIPT" claim $n --no-worktree >/dev/null; done
tip=$(git rev-parse master)
git push -q origin "$tip:refs/heads/issue-30" "$tip:refs/heads/issue-37"

expect 0 "reap <n> touches only n" withgh bash "$SCRIPT" reap 32
[ -z "$(claim_of 32)" ] && [ -n "$(claim_of 30)" ] && [ -n "$(claim_of 33)" ] \
  && ok "the other closed issues' claims stay" || bad "the other closed issues' claims stay"
expect 1 "reap refuses a number that isn't one" withgh bash "$SCRIPT" reap 3x

# A PR that closes #30 (GitHub's list) and #31 (a commit), and only mentions #33.
printf '%s\n' "MERGED false issue-30 $tip" "closes #30" "Part of #33, see #32" "do it" "Fixes #31" >"$GH_PR"
: >"$GH_LOG"
expect 0 "merged --dry-run" withgh bash "$SCRIPT" merged 9 --dry-run
grep -q "would reap #30" "$TMP/out" && grep -q "would delete issue-30" "$TMP/out" \
  && [ -n "$(claim_of 30)" ] && [ -n "$(git ls-remote origin refs/heads/issue-30)" ] && ! grep -q "labels/claimed" "$GH_LOG" \
  && ok "the dry run names the claim and branch, and changes nothing" || bad "the dry run names the claim and branch, and changes nothing"
expect 0 "merged" withgh bash "$SCRIPT" merged 9
grep -q "PR #9 closes #30 #31" "$TMP/out" && ok "it reads the closing keywords and GitHub's list" || bad "it reads the closing keywords and GitHub's list"
[ -z "$(claim_of 30)" ] && [ -z "$(git ls-remote origin refs/heads/issue-30)" ] \
  && ok "the closed issue's claim and its issue-30 branch are gone" || bad "the closed issue's claim and its issue-30 branch are gone"
[ -n "$(claim_of 31)" ] && [ -n "$(claim_of 33)" ] \
  && ok "an issue still open, and one only mentioned, keep their claims" || bad "an issue still open, and one only mentioned, keep their claims"
unlabeled 30 && ok "the claimed label comes off #30" || bad "the claimed label comes off #30"

# The issue's own run comes second and finds the claim gone; it says so.
claim30=$(git commit-tree -m "claim #30" "$(git mktree </dev/null)")
git push -q origin "$claim30:refs/heads/claim/issue-30"
expect 0 "reap racing another run" withgh env GH_MEANWHILE="git -C '$REPO' push -q origin --delete claim/issue-30" bash "$SCRIPT" reap 30
grep -q "already released: #30" "$TMP/out" && ! grep -q "kept #30" "$TMP/out" \
  && ok "a claim deleted meanwhile reads as already released" || bad "a claim deleted meanwhile reads as already released"

# The number comes from the closing keyword, not the branch name; a branch
# that moved after the merge stays.
printf '%s\n' "MERGED false issue-37 $tip" "Closes #36" >"$GH_PR"
expect 0 "merged from issue-37 closing #36" withgh bash "$SCRIPT" merged 10
[ -z "$(claim_of 36)" ] && [ -n "$(git ls-remote origin refs/heads/issue-37)" ] \
  && ok "it reaps #36 and keeps the issue-37 branch" || bad "it reaps #36 and keeps the issue-37 branch"
git commit -q --allow-empty -m "after the merge" && git push -q origin HEAD:refs/heads/issue-31 && git reset -q --hard HEAD~1
printf '%s\n' "MERGED false issue-31 $tip" "Closes #31" >"$GH_PR"
printf '%s\n' "31 CLOSED 0" >"$GH_STATES"
expect 0 "merged with a moved branch" withgh bash "$SCRIPT" merged 11
[ -z "$(claim_of 31)" ] && [ -n "$(git ls-remote origin refs/heads/issue-31)" ] && grep -q "kept issue-31" "$TMP/out" \
  && ok "the claim goes, the branch with new work stays" || bad "the claim goes, the branch with new work stays"

# Unmerged, or closing nothing: every claim stays.
printf '%s\n' "CLOSED false issue-33 $tip" "Closes #33" >"$GH_PR"
printf '%s\n' "33 CLOSED 0" >"$GH_STATES"
expect 0 "merged on a PR closed unmerged" withgh bash "$SCRIPT" merged 12
[ -n "$(claim_of 33)" ] && grep -q "not merged: every claim stays" "$TMP/out" \
  && ok "it leaves the claim" || bad "it leaves the claim"
printf '%s\n' "MERGED false docs-typo $tip" "Part of #33" "touch up" >"$GH_PR"
expect 0 "merged on a PR that closes nothing" withgh bash "$SCRIPT" merged 13
[ -n "$(claim_of 33)" ] && grep -q "closes no issue" "$TMP/out" \
  && ok "it leaves every claim" || bad "it leaves every claim"
git push -q origin --delete claim/issue-33 issue-31 issue-37
for n in 30 31 32 33 36; do rm -f "$RECS/issue-$n"; done
unset CLAIM_REAP_GRACE_MINUTES

# With the GraphQL budget spent, land still opens the PR, or finds the one
# open, over REST, with the title and body gh pr create --fill gives (#180).
git worktree add -q "$TMP/pr-wt" -b issue_40-over-rest master
cd "$TMP/pr-wt"
bash "$SCRIPT" claim 40 --here >/dev/null
git commit -q --allow-empty -m "Open the PR over REST" -m "Closes #40"
: >"$GH_LOG"; : >"$GH_PULLS"; mkdir -p "$GH_FIELDS"
expect 0 "pr-mode land with GraphQL spent" withgh env GH_NO_GRAPHQL=1 CLAIM_LAND=pr bash "$SCRIPT" land 40 --verify true
grep -qx "opened https://github.com/o/r/pull/99" "$TMP/out" && ok "it opens the PR and prints its url" || bad "it opens the PR and prints its url"
! grep -qE "^(pr|issue) |^api graphql" "$GH_LOG" && ok "and spends no GraphQL" || bad "and spends no GraphQL"
field() { [ "$(cat "$GH_FIELDS/$1"; echo .)" = "$2." ] && ok "one commit: $1 is ${3:-$2}" || bad "one commit: $1 is ${3:-$2} (got: $(cat "$GH_FIELDS/$1"))"; }
field title "Open the PR over REST"
field body $'Closes #40\n' "the commit's body"
field head issue_40-over-rest
field base master

# Its PR is open now: a second land finds it and opens none.
echo "https://github.com/o/r/pull/99" >"$GH_PULLS"; rm -f "$GH_FIELDS"/*
git commit -q --allow-empty -m "Second step"
expect 0 "land again with its PR open" withgh env GH_NO_GRAPHQL=1 CLAIM_LAND=pr bash "$SCRIPT" land 40 --verify true
grep -qx "pushed issue_40-over-rest; its PR: https://github.com/o/r/pull/99" "$TMP/out" && [ -z "$(ls "$GH_FIELDS")" ] \
  && ok "it finds the open PR and opens no other" || bad "it finds the open PR and opens no other"
grep -qF "api -X GET repos/{owner}/{repo}/pulls -F head={owner}:issue_40-over-rest -f state=open" "$GH_LOG" \
  && ok "looking it up by owner:branch" || bad "looking it up by owner:branch"

# Several commits, and master merged in: the branch name for a title, a list
# of the commits for a body, and no merge in it.
: >"$GH_PULLS"
git -C "$REPO" commit -q --allow-empty -m "master moves" && git -C "$REPO" push -q origin master
git fetch -q origin master && git merge -q --no-edit origin/master
expect 0 "land several commits with GraphQL spent" withgh env GH_NO_GRAPHQL=1 CLAIM_LAND=pr bash "$SCRIPT" land 40 --verify true
field() { [ "$(cat "$GH_FIELDS/$1"; echo .)" = "$2." ] && ok "several commits: $1 is ${3:-$2}" || bad "several commits: $1 is ${3:-$2} (got: $(cat "$GH_FIELDS/$1"))"; }
field title "issue 40 over rest"
field body $'- **Open the PR over REST**\n- **Second step**\n' "a list of them, oldest first"

# reap reads issue states over REST when GraphQL is spent.
CLAIM_AGENT_ID=elsewhere-41 bash "$SCRIPT" claim 41 --no-worktree >/dev/null
printf '%s\n' "41 CLOSED 600" "40 OPEN -" >"$GH_STATES"
: >"$GH_LOG"
expect 0 "reap with GraphQL spent" withgh env GH_NO_GRAPHQL=1 bash "$SCRIPT" reap 41
[ -z "$(claim_of 41)" ] && unlabeled 41 && ok "it reaps the closed issue's claim over REST" || bad "it reaps the closed issue's claim over REST"
bash "$SCRIPT" release 40 >/dev/null
cd "$REPO"
git worktree remove -f "$TMP/pr-wt"

# claim --here drops a record that outlived its claim, rather than refusing.
printf 'ghost\n/nowhere\n' >"$RECS/issue-26"
cd "$TMP/app-wt"
expect 0 "claim --here over a record whose claim is gone" bash "$SCRIPT" claim 26 --here
[ "$(sed -n 2p "$RECS/issue-26")" = "$(git rev-parse --show-toplevel)" ] \
  && ok "the new record names this worktree" || bad "the new record names this worktree"
bash "$SCRIPT" release 26 >/dev/null
cd "$REPO"

# accept_issue_branch: an issue-<n> branch on origin (an older claim.py claim)
# holds the issue, and the session on that branch passes the guard.
echo "accept_issue_branch=1" >"$REPO/scripts/issue.conf"
git add scripts && git commit -q -m "conf" && git push -q origin master
git push -q origin master:refs/heads/issue-16
expect 2 "claim of an issue-branch-claimed issue exits 2" bash "$SCRIPT" claim 16 --no-worktree
bash "$SCRIPT" list >"$TMP/out"; grep -q "^#16 .*branch issue-16" "$TMP/out" && ok "list shows the issue-branch claim" || bad "list shows the issue-branch claim"
git worktree add -q "$TMP/repo-16" -b issue-16 origin/issue-16
expect 0 "guard: the issue-16 branch's own push passes" hook "$TMP/repo-16" "git push origin issue-16"
expect 2 "guard: an issue-17 branch without origin/issue-17 is denied" \
  bash -c "git -C '$REPO' worktree add -q '$TMP/repo-17' -b issue-17 master && $(declare -f hook); GUARD='$GUARD' hook '$TMP/repo-17' 'git push -u origin issue-17'"

# In chrischaps/Dashboard, the vendored copy must match the canonical kit.
KIT=$(dirname "$SCRIPT")/../skills/setup-claims/kit/scripts
if [ -d "$KIT" ]; then
  for f in issue.sh issue-guard.sh issue-session.sh issue-test.sh; do
    cmp -s "$(dirname "$SCRIPT")/$f" "$KIT/$f" && ok "scripts/$f matches the kit" \
      || bad "scripts/$f matches the kit (re-run skills/setup-claims/install.sh)"
  done
  WF=$(dirname "$SCRIPT")/../.github/workflows/release-claim.yml
  cmp -s "$WF" "$KIT/../workflows/release-claim.yml" && ok "release-claim.yml matches the kit" \
    || bad "release-claim.yml matches the kit (re-run skills/setup-claims/install.sh)"

  # Updating an earlier install whose .gitignore un-hides settings.json and next-issue (Flint's):
  # the close-epic skill it adds must not be left hidden.
  if command -v node >/dev/null; then
    git init -q "$TMP/old-install"
    (
      cd "$TMP/old-install" || exit 1
      printf '%s\n' '.claude/*' '!.claude/settings.json' '!.claude/skills/' '.claude/skills/*' '!.claude/skills/next-issue/' >.gitignore
      mkdir -p .claude/skills/next-issue && echo own >.claude/skills/next-issue/SKILL.md
    )
    expect 0 "install.sh updates an earlier install" bash "$KIT/../../install.sh" "$TMP/old-install" --base master
    for f in .claude/settings.json .claude/skills/next-issue/SKILL.md .claude/skills/close-epic/SKILL.md; do
      git -C "$TMP/old-install" check-ignore -q "$f" && bad "install.sh leaves $f shipped" || ok "install.sh leaves $f shipped"
    done
    [ "$(cat "$TMP/old-install/.claude/skills/next-issue/SKILL.md")" = own ] \
      && ok "install.sh keeps the repo's own next-issue" || bad "install.sh keeps the repo's own next-issue"
  fi
fi

echo
[ "$fails" = 0 ] && echo "all passed" || { echo "$fails failed"; exit 1; }
