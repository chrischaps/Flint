# Changelog for games built on Flint

For a game that carries the engine as `engine/` and has just pulled a newer
one. The squash hides the engine's commit subjects, so the changelog is where
to read what changed and whether your game has to do anything. It lists what a
game can notice: scene and schema keys, the script API, CLI flags, file
formats, Cargo features, and changes in how something looks, sounds or
behaves. Each entry says **what changed** and **what to do**; "Nothing" means
old files and scripts keep working.

## Reading it after a pull

    bash engine/scripts/changelog.sh since 2026-11-01

prints every entry dated that day or later, newest first, including ones not
yet gathered into a month. Give it the day of your last engine pull
(`git log -1 --format=%cs -- engine`, run before pulling). The months, which
assembly lists here:

<!-- months: written by scripts/changelog.sh assemble -->
- [November 2026](changelog/2026-11.md)
<!-- /months -->

## Adding an entry

One file per pull request, holding its entry and nothing else, so two PRs
never touch the same file:

    bash scripts/changelog.sh new <issue> <slug>    # writes changelog.d/<issue>-<slug>.md

Fill it in and commit it with the change. After the merge the `Changelog`
workflow moves it into this month's file under the merge's date and deletes
it; don't edit `changelog/` or this page for a new entry. The format:

    ### `flint render --look` takes a path (#123)
    **What changed.** ...
    **What to do.** ...

Write it for someone who has never read the PR: name the key, function or
flag as a game author types it, say what happens to old files, and say what
to do. Keep it to the title and two short paragraphs, about 150 words; when
there is more, the book or the PR holds it and the entry links there.
`scripts/changelog.sh check` warns past 250.

A PR that changes `schemas/`, the script API (`crates/flint-script/src/api.rs`)
or the scene format (`crates/flint-scene/src/format.rs`) needs a fragment, and
CI's `changelog` job fails without one (`scripts/changelog-check.sh`). If no
game can notice the change, say so in a commit message on the branch instead,
on a line of its own:

    Changelog: none (reason)

Changes to CLI flags, Cargo features, file formats and defaults aren't
detected by the check. They still get an entry.
