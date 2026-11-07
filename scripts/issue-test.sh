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
  "api repos/{owner}/{repo} -q .default_branch") [ -n "${GH_NO_DEFAULT:-}" ] && exit 1; echo master ;;
  "api -X PATCH repos/{owner}/{repo}/issues/"*)
    [ -n "${GH_NO_CLOSE:-}" ] && exit 1
    n=${4##*/}; sed -i "s/^$n .*/$n CLOSED 0/" "$GH_STATES" ;;
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

# A branch stacked on a sibling's, and the sibling squash-merged (#223): the
# push carries the sibling's "Closes #50", which no claim here covers. It
# passes once #50 is closed on GitHub; while #50 is open, or GitHub can't be
# asked, it's refused as before.
git worktree add -q "$TMP/sib" -b issue-50 master
(cd "$TMP/sib" && echo a >s && git add s && git commit -q -m "Sibling, part one" -m "Closes #50" \
  && echo b >>s && git commit -q -am "Sibling, part two")
git worktree add -q "$TMP/stack" -b stacked issue-50
mkdir -p "$TMP/stack/scripts" && cp "$SCRIPT" "$TMP/stack/scripts/issue.sh"
(cd "$TMP/stack" && bash "$SCRIPT" claim 51 --here >/dev/null && git commit -q --allow-empty -m "Stacked" -m "Closes #51")
git merge -q --squash issue-50 && git commit -q -m "Sibling (#98)" && git push -q origin master
ghhook() { CLAIM_NO_GH= PATH="$TMP/bin:$PATH" hook "$@"; }
printf '%s\n' "50 OPEN -" >"$GH_STATES"
expect 2 "guard: push carrying an open sibling's Closes #50 is denied" ghhook "$TMP/stack" "git push"
grep -q '#50 is named by a commit this push carries ([0-9a-f]* "Sibling, part one")' "$TMP/out" \
  && grep -q "passes once #50 is closed" "$TMP/out" \
  && ok "the denial names the sibling's commit and what would let it pass" || bad "the denial names the sibling's commit"
expect 2 "guard: and so is it when GitHub can't be asked" hook "$TMP/stack" "git push"
printf '%s\n' "50 CLOSED 5" >"$GH_STATES"
: >"$GH_LOG"
expect 0 "guard: push carrying a squash-merged, closed sibling's Closes #50 passes" ghhook "$TMP/stack" "git push"
grep -q "i50:" "$GH_LOG" && ! grep -q "i51:" "$GH_LOG" \
  && ok "it asks GitHub about the unclaimed issue only" || bad "it asks GitHub about the unclaimed issue only"
expect 2 "guard: a commit naming the closed #50 is still denied" ghhook "$TMP/stack" 'git commit -m "Closes #50"'
# A sibling's commit whose patch is already on master passes with no GitHub call.
(cd "$TMP/stack" && echo c >u && git add u && git commit -q -m "Sibling 52" -m "Closes #52")
git cherry-pick -n stacked && git commit -q -m "Sibling 52 (#97)" && git push -q origin master
: >"$GH_LOG"
expect 0 "guard: push carrying a commit already on master passes" ghhook "$TMP/stack" "git push"
! grep -q "i52:" "$GH_LOG" && ok "without asking GitHub about it" || bad "without asking GitHub about it"
(cd "$TMP/stack" && bash "$SCRIPT" release 51 >/dev/null)
git worktree remove -f "$TMP/stack"; git worktree remove -f "$TMP/sib"; git branch -q -D stacked issue-50

# claim --here drops a record that outlived its claim, rather than refusing.
printf 'ghost\n/nowhere\n' >"$RECS/issue-26"
cd "$TMP/app-wt"
expect 0 "claim --here over a record whose claim is gone" bash "$SCRIPT" claim 26 --here
[ "$(sed -n 2p "$RECS/issue-26")" = "$(git rev-parse --show-toplevel)" ] \
  && ok "the new record names this worktree" || bad "the new record names this worktree"
bash "$SCRIPT" release 26 >/dev/null
cd "$REPO"

# A claim on a feature branch (#243). feature/x holds a commit naming #60, a
# sibling that merged into it and is still open: GitHub closes issues only for
# merges to the default branch.
git worktree add -q "$TMP/feat" -b feature/x master
(cd "$TMP/feat" && echo f >feat && git add feat && git commit -q -m "Sibling on the feature" -m "Closes #60" \
  && git push -q origin feature/x)
feat_tip=$(git rev-parse feature/x)
git worktree remove -f "$TMP/feat"
git branch -q -D feature/x
git update-ref -d refs/remotes/origin/feature/x
body_of() { git log -1 --format=%B "$(claim_of "$1")"; }
patched() { grep -q "api -X PATCH repos/{owner}/{repo}/issues/$1 " "$GH_LOG"; }

expect 1 "claim --base with no branch is refused" bash "$SCRIPT" claim 61 --base
expect 1 "claim --base with an option for a branch is refused" bash "$SCRIPT" claim 61 --base --here
expect 1 "claim --base of a branch not on origin is refused" bash "$SCRIPT" claim 61 --base feature/nope
grep -q "feature/nope isn't a branch on origin" "$TMP/out" && [ -z "$(claim_of 61)" ] && [ ! -e "$TMP/repo-61" ] \
  && ok "and claims nothing" || bad "and claims nothing"

expect 0 "claim --base" bash "$SCRIPT" claim 61 --base feature/x
[ "$(git -C "$TMP/repo-61" rev-parse HEAD)" = "$feat_tip" ] && [ "$(git -C "$TMP/repo-61" branch --show-current)" = issue-61 ] \
  && ok "issue-61 branches from the feature branch" || bad "issue-61 branches from the feature branch"
grep -qx "base: feature/x" <<<"$(body_of 61)" && ok "the claim commit records the base" || bad "the claim commit records the base"
bash "$SCRIPT" list >"$TMP/out"
grep -q "^#61 .*ago  on feature/x$" "$TMP/out" && ok "list shows the base" || bad "list shows the base"

# A plain claim is what it always was: no trailer, the repo's base.
expect 0 "a plain claim" bash "$SCRIPT" claim 62 --no-worktree
! grep -q "^base:" <<<"$(body_of 62)" && ok "records no base" || bad "records no base"
[ "$(bash "$SCRIPT" base 62)" = master ] && ok "base <n> of a plain claim is the repo's" || bad "base <n> of a plain claim is the repo's"
[ "$(bash "$SCRIPT" base 61)" = feature/x ] && ok "base <n> reads the claim" || bad "base <n> reads the claim"
[ "$(CLAIM_BASE=feature/x bash "$SCRIPT" base 62)" = feature/x ] && [ "$(CLAIM_BASE=master bash "$SCRIPT" base 61)" = feature/x ] \
  && ok "CLAIM_BASE fills in for a claim with none, and doesn't override one" || bad "CLAIM_BASE fills in for a claim with none, and doesn't override one"
[ "$(bash "$SCRIPT" base)" = master ] && ok "base from a worktree holding no claim is the repo's" || bad "base from a worktree holding no claim is the repo's"
[ "$(cd "$TMP/repo-61" && bash "$SCRIPT" base)" = feature/x ] && ok "base from the claim's worktree is the claim's" || bad "base from the claim's worktree is the claim's"
bash "$SCRIPT" release 62 >/dev/null

# Renewing and stealing keep the base.
(cd "$TMP/repo-61" && bash "$SCRIPT" renew 61 >/dev/null)
grep -qx "base: feature/x" <<<"$(body_of 61)" && ok "renew keeps the base" || bad "renew keeps the base"
CLAIM_AGENT_ID=holder-63 bash "$SCRIPT" claim 63 --no-worktree --base feature/x >/dev/null
rm -f "$RECS/issue-63"
expect 0 "a stale feature claim is stolen" env CLAIM_TTL_HOURS=0 CLAIM_AGENT_ID=taker-63 bash "$SCRIPT" claim 63 --steal --no-worktree
grep -qx "base: feature/x" <<<"$(body_of 63)" && grep -qx "stolen-from: holder-63" <<<"$(body_of 63)" \
  && ok "the steal keeps the base" || bad "the steal keeps the base"
expect 0 "a steal can move the base" env CLAIM_TTL_HOURS=0 CLAIM_AGENT_ID=taker-63 bash "$SCRIPT" claim 63 --steal --no-worktree --base master
grep -qx "base: master" <<<"$(body_of 63)" && ok "to the one it names" || bad "to the one it names"
CLAIM_AGENT_ID=taker-63 bash "$SCRIPT" release 63 >/dev/null

# The guard judges a push against the claim's base: #60 is on feature/x, so it
# isn't carried. Against the repo's base it would be, and it's open.
mkdir -p "$TMP/repo-61/scripts" && cp "$SCRIPT" "$TMP/repo-61/scripts/issue.sh"
(cd "$TMP/repo-61" && echo w >w && git add w && git commit -q -m "Feature work" -m "Closes #61")
printf '%s\n' "60 OPEN -" "61 OPEN -" >"$GH_STATES"
expect 0 "guard: a push on the claim's base passes a sibling's Closes already on it" ghhook "$TMP/repo-61" "git push"
(cd "$TMP/repo-61" && git commit -q --allow-empty -m "Stowaway" -m "Closes #64")
expect 2 "guard: a Closes past the claim's base is still denied" ghhook "$TMP/repo-61" "git push"
grep -q "#64 is named by a commit" "$TMP/out" && ! grep -q "#60" "$TMP/out" \
  && ok "naming that one and not the base's" || bad "naming that one and not the base's"
(cd "$TMP/repo-61" && git reset -q --hard HEAD~1)
# The control: the same history with no base on its claim is refused over #60,
# from a --here worktree whose branch names no issue, as it was before.
git worktree add -q "$TMP/plain" -b plain-on-feature origin/feature/x
mkdir -p "$TMP/plain/scripts" && cp "$SCRIPT" "$TMP/plain/scripts/issue.sh"
(cd "$TMP/plain" && bash "$SCRIPT" claim 65 --here >/dev/null)
expect 2 "guard: the same history on a claim with no base is denied" ghhook "$TMP/plain" "git push"
grep -q "#60 is named by a commit" "$TMP/out" && ok "over the sibling's #60" || bad "over the sibling's #60"
(cd "$TMP/plain" && bash "$SCRIPT" release 65 >/dev/null)
(cd "$TMP/plain" && bash "$SCRIPT" claim 65 --here --base feature/x >"$TMP/out" 2>&1)
! grep -q "warning" "$TMP/out" && ok "claim --here --base on the base gives no warning" || bad "claim --here --base on the base gives no warning"
expect 0 "guard: and with --here --base it passes, on any branch name" ghhook "$TMP/plain" "git push"
# Two claims in one worktree on different bases say nothing about a push.
(cd "$TMP/plain" && bash "$SCRIPT" claim 66 --here >/dev/null)
[ "$(cd "$TMP/plain" && bash "$SCRIPT" base)" = master ] && ok "claims here that disagree give the repo's base" || bad "claims here that disagree give the repo's base"
(cd "$TMP/plain" && bash "$SCRIPT" release 65 >/dev/null && bash "$SCRIPT" release 66 >/dev/null)
git worktree remove -f "$TMP/plain"; git branch -q -D plain-on-feature
# --here warns when the worktree isn't built on the base it names.
(cd "$TMP/app-wt" && bash "$SCRIPT" claim 67 --here --base feature/x >"$TMP/out" 2>&1)
grep -q "warning: HEAD isn't built on origin/feature/x" "$TMP/out" && ok "claim --here --base off the base warns" || bad "claim --here --base off the base warns"
(cd "$TMP/app-wt" && bash "$SCRIPT" release 67 >/dev/null)

# land opens the PR to the claim's base, with only this work in it.
cd "$TMP/repo-61"
: >"$GH_LOG"; : >"$GH_PULLS"; rm -f "$GH_FIELDS"/*
expect 0 "pr-mode land on a feature base" withgh env CLAIM_LAND=pr bash "$SCRIPT" land 61 --verify true
field() { [ "$(cat "$GH_FIELDS/$1"; echo .)" = "$2." ] && ok "feature land: $1 is ${3:-$2}" || bad "feature land: $1 is ${3:-$2} (got: $(cat "$GH_FIELDS/$1"))"; }
field base feature/x
field head issue-61
field title "Feature work"
[ "$(git rev-parse origin/master)" != "$(git rev-parse HEAD)" ] && [ "$(git rev-parse origin/feature/x)" = "$feat_tip" ] \
  && ok "and leaves both branches where they were" || bad "and leaves both branches where they were"
[ -n "$(claim_of 61)" ] && ok "and keeps the claim" || bad "and keeps the claim"
[ "$(git diff --name-only origin/feature/x HEAD)" = w ] \
  && ok "the PR carries the work, and no change to issue.conf" || bad "the PR carries the work, and no change to issue.conf"
cd "$REPO"

# merged: a PR into the claim's base closes the issue, then releases the claim.
export CLAIM_REAP_GRACE_MINUTES=0
tip61=$(git -C "$TMP/repo-61" rev-parse HEAD)
printf '%s\n' "MERGED false issue-61 $tip61 feature/x" "Closes #61" >"$GH_PR"
printf '%s\n' "61 OPEN -" >"$GH_STATES"
: >"$GH_LOG"
expect 0 "merged --dry-run on a feature base" withgh bash "$SCRIPT" merged 70 --dry-run
grep -q "would close #61: PR #70 merged into feature/x" "$TMP/out" && ! patched 61 && [ -n "$(claim_of 61)" ] \
  && grep -q "61 OPEN" "$GH_STATES" && ok "the dry run names the close, and changes nothing" || bad "the dry run names the close, and changes nothing"
expect 0 "merged on a feature base" withgh bash "$SCRIPT" merged 70
patched 61 && grep -q "issues/61 -f state=closed -f state_reason=completed" "$GH_LOG" \
  && ok "it closes the issue" || bad "it closes the issue"
grep -E "issues/61/comments -f body=.*PR #70.*feature/x" "$GH_LOG" >/dev/null \
  && ok "with a comment naming the PR and the base" || bad "with a comment naming the PR and the base"
[ -z "$(claim_of 61)" ] && unlabeled 61 && ok "and releases the claim and its label" || bad "and releases the claim and its label"
[ -z "$(git ls-remote origin refs/heads/issue-61)" ] && ok "and deletes its issue-61 branch" || bad "and deletes its issue-61 branch"
# The workflow runs again, or GitHub's own close comes after: nothing said twice.
: >"$GH_LOG"
expect 0 "merged again" withgh bash "$SCRIPT" merged 70
! patched 61 && ! grep -q "issues/61/comments" "$GH_LOG" && ok "closes and comments nothing twice" || bad "closes and comments nothing twice"
git worktree remove -f "$TMP/repo-61"; git branch -q -D issue-61

# A PR that isn't the claim's closes nothing: a claim with no base, a claim on
# another base, and an issue nobody claimed all stay open, and claims stay.
CLAIM_AGENT_ID=e bash "$SCRIPT" claim 71 --no-worktree >/dev/null
CLAIM_AGENT_ID=e bash "$SCRIPT" claim 72 --no-worktree --base feature/x >/dev/null
printf '%s\n' "71 OPEN -" "72 OPEN -" "73 OPEN -" >"$GH_STATES"
printf '%s\n' "MERGED false docs $tip feature/y" "Closes #71" "Closes #72" "Closes #73" >"$GH_PR"
: >"$GH_LOG"
expect 0 "merged into a branch no claim names" withgh bash "$SCRIPT" merged 74
grep -q "left #71 open: .*its claim's base is not recorded" "$TMP/out" && grep -q "left #72 open: .*its claim's base is feature/x" "$TMP/out" \
  && grep -q "left #73 open: .*it has no claim" "$TMP/out" && ok "it says why each stays open" || bad "it says why each stays open"
! grep -q "api -X PATCH" "$GH_LOG" && [ -n "$(claim_of 71)" ] && [ -n "$(claim_of 72)" ] \
  && ok "closes nothing and keeps the claims" || bad "closes nothing and keeps the claims"

# A merge to the default branch is left to GitHub, base recorded or not.
printf '%s\n' "MERGED false docs $tip master" "Closes #71" "Closes #72" >"$GH_PR"
: >"$GH_LOG"
expect 0 "merged into the default branch" withgh bash "$SCRIPT" merged 75
! grep -q "api -X PATCH" "$GH_LOG" && [ -n "$(claim_of 71)" ] && [ -n "$(claim_of 72)" ] \
  && ok "closes nothing itself, and the claims wait for GitHub's close" || bad "closes nothing itself, and the claims wait for GitHub's close"
printf '%s\n' "71 CLOSED 0" "72 CLOSED 0" >"$GH_STATES"
expect 0 "merged once GitHub has closed them" withgh bash "$SCRIPT" merged 75
[ -z "$(claim_of 71)" ] && [ -z "$(claim_of 72)" ] && ok "then it reaps them, as before" || bad "then it reaps them, as before"
# Even a claim that names the default branch as its base: that close is GitHub's.
CLAIM_AGENT_ID=e bash "$SCRIPT" claim 80 --no-worktree --base master >/dev/null
printf '%s\n' "80 OPEN -" >"$GH_STATES"
printf '%s\n' "MERGED false docs $tip master" "Closes #80" >"$GH_PR"
: >"$GH_LOG"
expect 0 "merged into the default branch, its claim's base" withgh bash "$SCRIPT" merged 81
! grep -q "api -X PATCH" "$GH_LOG" && ! grep -q "issues/80/comments" "$GH_LOG" && [ -n "$(claim_of 80)" ] \
  && ok "closes and comments nothing, and the claim waits" || bad "closes and comments nothing, and the claim waits"
CLAIM_AGENT_ID=e bash "$SCRIPT" release 80 >/dev/null

# GitHub can't say which branch is the default: nothing is closed on a guess.
CLAIM_AGENT_ID=e bash "$SCRIPT" claim 76 --no-worktree --base feature/x >/dev/null
printf '%s\n' "76 OPEN -" >"$GH_STATES"
printf '%s\n' "MERGED false docs $tip feature/x" "Closes #76" >"$GH_PR"
: >"$GH_LOG"
expect 0 "merged with no default branch known" withgh env GH_NO_DEFAULT=1 bash "$SCRIPT" merged 77
! grep -q "api -X PATCH" "$GH_LOG" && [ -n "$(claim_of 76)" ] && ok "closes nothing, keeps the claim" || bad "closes nothing, keeps the claim"
# A close that fails keeps the claim and fails the run; running it again finishes.
expect 1 "merged when the close fails" withgh env GH_NO_CLOSE=1 bash "$SCRIPT" merged 77
[ -n "$(claim_of 76)" ] && ! grep -q "issues/76/comments" "$GH_LOG" \
  && ok "keeps the claim and comments nothing" || bad "keeps the claim and comments nothing"
expect 0 "merged again once it can" withgh bash "$SCRIPT" merged 77
[ -z "$(claim_of 76)" ] && ok "releases the claim" || bad "releases the claim"
unset CLAIM_REAP_GRACE_MINUTES

# Push mode on a feature base: land moves that branch, and closes the issue,
# since nothing else would. On the default branch it leaves the close to GitHub.
printf '%s\n' "78 OPEN -" "79 OPEN -" >"$GH_STATES"
bash "$SCRIPT" claim 78 --base feature/x >/dev/null
cd "$TMP/repo-78"
git commit -q --allow-empty -m "Pushed to the feature" -m "Closes #78"
: >"$GH_LOG"
expect 0 "push-mode land on a feature base" withgh bash "$SCRIPT" land 78 --verify true
[ "$(git rev-parse origin/feature/x)" = "$(git rev-parse HEAD)" ] && ok "it moves the feature branch" || bad "it moves the feature branch"
[ "$(git rev-parse origin/master)" != "$(git rev-parse HEAD)" ] && ok "and not the default branch" || bad "and not the default branch"
patched 78 && grep -q "issues/78/comments .*feature/x" "$GH_LOG" && [ -z "$(claim_of 78)" ] \
  && ok "closes the issue, says where, and releases the claim" || bad "closes the issue, says where, and releases the claim"
cd "$REPO"
git worktree remove -f "$TMP/repo-78"; git branch -q -D issue-78
bash "$SCRIPT" claim 79 >/dev/null
cd "$TMP/repo-79"
git commit -q --allow-empty -m "Pushed to master" -m "Closes #79"
: >"$GH_LOG"
expect 0 "push-mode land with no base recorded" withgh bash "$SCRIPT" land 79 --verify true
! grep -q "api -X PATCH" "$GH_LOG" && ! grep -q "default_branch" "$GH_LOG" && [ "$(git rev-parse origin/master)" = "$(git rev-parse HEAD)" ] \
  && ok "lands on master and leaves the close to GitHub" || bad "lands on master and leaves the close to GitHub"
cd "$REPO"
git fetch -q origin master && git merge -q --ff-only origin/master
git worktree remove -f "$TMP/repo-79"; git branch -q -D issue-79

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
