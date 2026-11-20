# 0131: The examples are a project of their own, in flint-examples

Date: 2026-11-20. Issue: #984 (epic #676). Follows [ADR 0122](0122-what-a-games-engine-carries.md), decision 1.

## Context

A game carries the engine as a squashed subtree, which is the whole tree.
ADR 0122 put that tree under a budget and decided the direction for the
larger split: the examples, 45 MB of the 169, move to their own repository.
Since #763 no golden or CLI test loaded an example, but the tree still
reached into them in four ways: the frozen fixtures borrowed 26 example
binaries by path (pinned by SHA-256 in `tests/fixtures/borrowed.toml`), a
dozen tests read example scenes, models and styles directly, the showcase
game named the examples' YBot, and the book, the CLAUDE.md files and code
comments cited about 750 example paths. The showcase film's rig
(`tools/showcase/`) captured scenes that live only in the examples.

Two questions came with the move: how the examples get an engine, and how
engine work that comes with an example bench is done across two
repositories. `flint-demos` (2026-11-17) had already answered the first
for whole films and places: the engine as a squashed subtree at `engine/`,
a `flint.toml` naming its schemas, `flint examples --dir demos --check`.

## Decision

1. **flint-examples is a Flint project like any game.** The engine is a
   squashed subtree at `engine/`, pulled from Flint's `main`
   (`git subtree pull --prefix=engine flint-engine main --squash`); its
   `flint.toml` names `engine/schemas` and `engine/schemas/examples` and the
   collision layers its scenes use; its CI builds the engine's headless CLI
   from the subtree and runs `flint examples --check`. The repository was
   made with `git subtree split --prefix=examples`, so the examples keep
   their history since #763 (64 commits; before that they were `demo/`).
2. **The scenes sit at the repository's root**, not under an `examples/`
   folder inside it. So `index.toml`, `bundle.toml` and the asset folders
   are the project root's, a path a script names from the root
   (`load_scene("showcase_crucible.scene.toml")`) is short, and the one way
   to cite an example anywhere is `flint-examples/<path>`.
3. **`flint-examples/<path>` is a real path.** A clone of the repository
   under its default name inside a Flint checkout (`.gitignore` lists
   `/flint-examples/`) makes every citation in the book and the CLAUDE.md
   files a path from the checkout's root: `flint play
   flint-examples/phase4_runtime.scene.toml` works there as written, and the
   project is discovered from the scene, so the clone's own `flint.toml`
   applies. The book links each cited file on GitHub.
4. **Checked both ways.** `scripts/examples-check.sh` (in `scripts/ci.sh`)
   fails a tracked `examples/` folder and any path to one (not
   `schemas/examples`, `specs/examples` or a crate's own cargo `examples/`,
   which no such path can be written as). `scripts/docs-check.py` holds every
   `flint-examples/<path>` and every link into the repository, in the book,
   the README and the CLAUDE.md files, to a file or folder there (a clone if
   one is present, else GitHub's API; CI passes its token).
5. **The fixtures own what they load.** They took back the 26 binaries they
   borrowed (19.2 MB, the same bytes, named `./models/YBot.glb` from the
   scene), `borrowed.toml` and its check are gone, and every test that read
   an example reads a frozen copy under `tests/fixtures/scenes/` or builds
   its own. The showcase game and Sun Prints keep their own copies of what
   they shared with the examples.
6. **An engine feature with an example bench is two pull requests**: the
   engine's first, with its tests on fixtures, then flint-examples', which
   pulls the merged engine and adds the bench. flint-examples renaming or
   deleting a file the engine's docs cite fails the engine's docs-check
   until the citation follows, which is the coupling on purpose: the book
   never points at nothing.
7. **The film rig went with its scenes.** `tools/showcase/` (the shot list,
   the cuts, the score) and its storyboard (`docs/design/showcase-film.md`,
   now `flint-examples/tools/showcase/storyboard.md`) are flint-examples'.
   `flint capture` stays the engine's, and the book's filming guide stays in
   the book, citing the rig where it now lives.

## Alternatives considered

- **An `examples/` folder inside flint-examples**, as flint-demos has
  `demos/`. It would have left the 178 files that named `examples/...`
  untouched, but every citation would read
  `flint-examples/examples/pbr_test.scene.toml`, and a clone inside the
  checkout would sit beside a guard that refuses the path `examples/`.
- **Examples into flint-demos.** One repository fewer, but the demos are
  films and places to watch; the examples are one feature each, the
  engine's documentation in scene form, and their index and CI are their
  own.
- **The examples pinning no engine** (built against whatever Flint a
  reader has). Nothing to pull, but no CI could say which engine an example
  works with, and a game's pattern is the one the book already teaches.
- **The engine's CI checking out flint-examples** and running the old
  `flint examples --check` against this tree's engine. It would catch an
  engine change that breaks an example on the PR that makes it, but every
  engine PR would then depend on a second repository's state; the examples'
  own CI catches it at the next subtree pull, which is when it matters to
  them.

## Consequences

- A game's `engine/` measures 158.0 MB, not 181.4 (ADR 0122's Measured
  table): 45.3 MB of examples left, 19.2 MB came back as the fixtures' own
  files and 2.4 MB as the showcase game's. The whole-tree budget is 175 MB.
  The sparse recipe drops its `engine/examples` line.
- `flint examples` with no `--dir` reads the project root's `index.toml`
  (in a flint-examples checkout, its own); the engine's CI no longer runs
  it, flint-examples' does.
- Running an example from a Flint checkout uses the clone's subtree schemas.
  When the engine's have moved ahead, pass
  `--schemas schemas --schemas schemas/examples`.
- The play site packed no example (its games are their own repositories
  and Sun Prints), so it did not change.
- The first subtree in flint-examples was taken from this change's branch,
  before the merge; the next `git subtree pull` from `main` brings it level.
