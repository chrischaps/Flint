---
name: close-epic
description: Close out an epic (an issue labelled `epic`, or any issue with sub-issues) once every sub-issue is closed. Checks each box of the epic's own "Done when" against the real, running system end to end, files a new sub-issue for anything that fails, makes the milestone presentation and Vault note, and closes the epic only when its done-when holds. Use when asked to "close out #N", "close the epic", "/close-epic N", or when next-issue finds an epic whose sub-issues are all closed.
---

# close-epic

An epic is a tracking issue. Its sub-issues are claimed, built and landed one at a time, each against its own tests. Nobody checks the epic's own "Done when" along the way, and GitHub doesn't close a parent when its children close. So when the last sub-issue lands, the epic is **ready to close out**, not done: the seams between the tickets have never been tried together.

Close-out is that one pass. It verifies; it doesn't build. Anything that fails becomes a new sub-issue, picked up like any other ticket.

## 1. Claim it and check it's ready

```bash
scripts/issue.sh claim <epic> --here    # or: scripts/issue.sh claim <epic>
gh issue view <epic> --comments
gh api repos/{owner}/{repo}/issues/<epic>/sub_issues --paginate --jq '.[] | "#\(.number) \(.state) \(.title)"'
```

Only one agent closes out an epic, so claim it like any issue. If any sub-issue is still open, it isn't ready: say which are open, release the claim (`scripts/issue.sh release <epic>`), and stop. Read the whole epic, its comments, and the closing comments and PRs of its sub-issues. Check Claude's Vault for notes on its milestone.

## 2. Try every "Done when" box for real

For each box in the epic's **Done when**, write down how you'll test it, then do exactly that against the real system:

- **The running app, from the user's side.** Start it on current main (the base branch, freshly pulled), not a sub-issue's branch. Drive it as the user would: the browser pane, the CLI, the skill in a plain session. A box that says "from Signal" means clicking in Signal, not calling the route the click calls.
- **Real data, not fixtures.** The real repo, the real GitHub, a real screenshot. Fixtures are where tickets agree with themselves.
- **Somewhere it wasn't built.** If the feature serves other projects, try one of those too. Installed copies, paths and config differ from the repo the work happened in.
- **The whole path.** A box that spans tickets (request → proposal → filed issue) is tested across all of them in one go. That is the point of the pass.
- **Keep the evidence.** Screenshots, URLs of what got made, the log lines. They go into the presentation.

The sub-issues' tests passing is not evidence for any box. Their done-whens were about each piece; the epic's is about the whole.

## 3. File what fails, as sub-issues

For each box that fails, find the cause far enough to describe it, then file a bug and link it to the epic:

```bash
gh issue create --title "<what fails, said as the user sees it>" --label bug --body "<steps, expected, actual, evidence, suspected seam>"
id=$(gh api repos/{owner}/{repo}/issues/<new> --jq .id)
gh api -X POST repos/{owner}/{repo}/issues/<epic>/sub_issues -F sub_issue_id=$id
```

Don't fix them inside the close-out. Release the epic's claim with a comment listing the boxes that pass, the ones that fail and the sub-issues filed. The epic goes back to having open sub-issues; when they land, close-out runs again. (A one-line fix you're sure of can go in its own PR, linked as a sub-issue the same way. Then go on with the close-out after it merges.)

## 4. Present and leave a trail

Once every box holds:

- **The milestone presentation** the epic asks for (most do: "a presentation for the whole milestone"). One page for the whole feature, built from the evidence of step 2: the story of what a user can now do, end to end, with real captures. Link each sub-issue's own presentation for the detail.
- **A Vault note** for the milestone: what it does, what close-out found, and what the next agent should know.

Then tick each box on the epic (`gh issue edit <epic> --body-file …`), post a closing comment with the evidence per box and the presentation link, and close it:

```bash
gh issue close <epic> --reason completed
scripts/issue.sh release <epic>
```

## Worked example: Request Work (chrischaps/Dashboard #101)

#101 was filed as an epic with seven sub-issues (#102–#108) and blocked-by links between them. Seven agent sessions each claimed one, built it, tested it and landed it. By the end the epic read 7/7, every sub-issue closed and green.

Filing from Signal still didn't work. The first real proposal Chris tried to file failed at its first Done-when box ("a request with a screenshot becomes a filed bug with the screenshot visible on GitHub"), at seams no single ticket owned:

- **The repo name's case** (PR #127). The skill (#105) writes the project as `gh` spells it, `chrischaps/Dashboard`. Signal's project ids are lowercase. The filing route (#106) compared them exactly, so it refused every real proposal for a repo with a capital letter. Its tests registered the project in a spelling no real scan produces.
- **The image id's format** (PR #129). The skill (#105) names an image by its file name without the extension. The upload store (#104) only accepted `<sha256>.<ext>`. Each was right by its own ticket, and the screenshot "wasn't on this machine" while sitting in `~/.signal/uploads`.
- **The fix's own seam** (also PR #129). #127 made the filing run carry the lowercase id, but the proposal panel (#108) still matched runs by the proposal's spelling, so it never saw its run and showed neither progress nor the error. #127's tests covered the route, not the panel.

All three are what close-out exists to catch: each sub-issue's tests used its own fixtures, which agreed with themselves. Only clicking File in Signal's panel on a real proposal, with a real screenshot, through to a real issue on GitHub, crossed every seam.

The close-out pass itself (PR #136) then ran each box against a project that isn't Signal, The Capitol: a real screenshot of the game, a large request that became an epic, and `/request-work` from a plain session there. It found two more gaps that no sub-issue's tests could have seen:

- **The installed copy drifts.** Every project but Signal runs the globally installed copy of the skill, and installing never overwrote it. On Chris's machine it was missing #106's filer and the #129 fix.
- **A plain session couldn't file a screenshot.** The command-line filer had the same bare-id bug #129 fixed in signald: one bug, two code paths, and the fix only reached the one under test.

It also says what it didn't do: no real issues filed into the game's backlog, and the installed copy not yet updated. That is the shape of a good close-out comment. (#136 fixed both gaps in the close-out PR. Under step 3 each would be its own sub-issue, which keeps the close-out a check rather than a build. A small, sure fix in the same PR is a judgment call; say so when you make it.)
