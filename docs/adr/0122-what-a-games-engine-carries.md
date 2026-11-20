# ADR 0122: What a game's `engine/` carries: one tree under a budget, no binary twice

Status: accepted, 2026-11-14 (#759). Decision 1's split was made on 2026-11-20 (#984, [ADR 0131](0131-the-examples-are-a-project-of-their-own.md)): see "The examples leave" under Measured. The engine area's budget moved on 2026-11-20 (#1303): see "The engine area grows by its code".

## Context

A game carries the engine as a subtree (`git subtree add --prefix=engine ...
--squash`, [Building a Game Project](../book/src/guides/building-a-game-project.md)),
and a subtree is the whole tree. So every byte tracked here is a byte in every
game clone, every game worktree and every CI checkout of a game, and in every
agent worktree of the engine itself (#762).

On 2026-11-14, before this change, the tree was 390 MB in 2,187 files:
the examples 130 MB, `docs/` 89 MB, `tests/` 78 MB, `testModels/` 37 MB,
`crates/` 38 MB, `showcase_game/` 16 MB. Only 275 MB of that was distinct
bytes: 54 groups of byte-identical files held 115 MB. `testModels/`
duplicated the examples' bench and YBots, and #763 had copied 40 example
binaries into `tests/fixtures/scenes/` on purpose, so the goldens stay frozen
while the examples improve. The largest single files were textures: the
wooden bench at 4096 texels (32 MB, three copies), `wood_planks.png` at 4096
(28 MB, two copies, and a PNG re-save of `worn_planks_diff_4k.jpg`, which no
scene read). Unread too: a TIFF the `image` dependency can't decode, four of
the five Poliigon concrete maps, `mug2.glb`, `CubeWithTexture.glb`. The
book's PNGs were saved with fast compression.

The issue asked for three things: no binary tracked twice, the largest
textures under an agreed cap, and a written budget for what a game's `engine/`
carries, with a decision on moving the heavy part out of the subtree.

## Decision

1. **One tree, one subtree, for now.** No Git LFS, no CI-published game
   branch, no second repository in this change. The larger split is decided
   in direction: **examples move to a `flint-examples` repository**, pulled
   separately, as its own issue (#984, under epic #676). Since #763 no golden or CLI
   test loads an example, so the move costs the engine's CI little. What it
   costs is cross-repo work: a feature PR that adds an example bench becomes
   two PRs, and the book's citations of the examples, `flint examples --check`,
   `flint export web` and the play site need to point across.
2. **A budget, checked on every PR.** `scripts/size-check.py` (standard
   library; `scripts/ci.sh` runs it, so CI's rustfmt job and `issue.sh land`
   do too) holds the tree to:

   | What | Budget | Measured at the decision |
   |---|---|---|
   | The whole tree, which a game's `engine/` is | 185 MB, 175 since #984 | 168.8 MB; 158.0 at #984 |
   | the examples (gone since #984) | 52 MB | 45.3 MB |
   | the book (`docs/`) | 78 MB | 70.2 MB |
   | tests (`tests/`, the golden PNGs) | 27 MB, 50 since #984 | 23.4 MB; 45.7 at #984 |
   | `showcase_game/` | 16 MB, 18 since #984 | 13.8 MB; 16.2 at #984 |
   | the engine proper (crates, schemas, styles, scripts, tools) | 20 MB, 23 since #1303 | 16.1 MB; 20.5 at #1303 |
   | one file | 8 MB | `MetalRoughSpheres.glb` excepted (11 MB of Khronos geometry) |
   | one texture, loose or in a `.glb` | 2048 texels a side, 3 MB | |

   The budgets are the measurement plus about a tenth. A breach is a reason
   to shrink something or to argue the budget here, not to raise it in
   passing.
3. **No binary is tracked twice.** The check hashes every tracked binary.
   The frozen fixtures **borrow** the example files they share, by path
   (`asset = "../../../<the examples>/models/YBot.glb"`; the asset search's
   as-written step, ADR 0114), and `tests/fixtures/borrowed.toml` pins each
   one's SHA-256. The check fails when a pinned file changes, when a fixture
   names an example binary that isn't pinned, and when a pin has no
   borrower left. Changing a borrowed example file is copy on write: first
   copy the old bytes into the fixtures and point the fixture there, then
   change the example. Three kinds of copy are kept on purpose, each named
   with its reason in `DUPLICATE_EXCEPTIONS`: Sun Prints' 10 KB palm (a web
   game whose bundle packs only its own folder), the showcase game's ten
   sounds (#760; its scripts build their names at run time, inside its own
   project), and five images of the frozen frame captures (#1087, 2.5 KB: a
   pass the diffed entity never reached is the same image in both captures,
   and each capture's manifest names its images inside its own folder).
4. **Textures are capped** at 2048 texels a side and 3 MB encoded.
   `tools/assets/cap_textures.py` halves an image over the side cap with
   `flint_pixels::mip_chain`'s own filter (2x2 box, colour in linear light),
   so the result is the renderer's next mip level up to one rounding; it
   writes colour maps (glTF base colour, emissive) as JPEG at quality 92 with
   no chroma subsampling, keeps data maps (normal, metallic-roughness,
   occlusion) PNG, drops alpha only where every material using it is
   `OPAQUE`, and rewrites a GLB's chunks and buffer views whole. Book images
   and golden renders are pictures, not textures, and are not held to it; a
   PNG anywhere is saved compressed. A book PNG has no alpha channel (#1276):
   a render or screenshot is saved RGBA with every pixel opaque, and
   `tools/assets/pack_book_images.py` drops the channel with the pixels
   unchanged. `ALPHA_EXCEPTIONS` in the check names one whose page needs
   the transparency.
5. **A game's worktrees can leave the heavy part out.** A sparse checkout
   keeps `engine/examples`, `engine/docs`, `engine/tests` and
   `engine/showcase_game` out of a game's working tree. Nothing a game
   builds reads them (no `include_bytes!` or build script reaches outside
   the crates), the engine workspace still resolves, and `git subtree pull`
   still works. The recipe is in the game-project guide.

## Alternatives considered

- **Keep #763's fixture copies, exempt `tests/fixtures/`.** The simplest
  rule, and about 15 MB of duplicates after the textures were capped. Turned
  down: the duplicate rule would have a directory-sized hole, and the copies
  would still double whatever an example and a fixture share next.
- **A shared root asset pool** both find through the asset search. The walk
  reaches the root from both the examples and `tests/fixtures/scenes/`, but a
  game's search also falls back to `engine/`, so every game would see those
  models.
- **A CI-published game branch** (main without examples, docs and tests).
  Games would carry about 40 MB, but `git subtree push` back to `main`, which
  games do (Vault: Git Subtree Branch Conventions), would push trees missing
  those folders.
- **Git LFS.** A subtree copies the pointer files, which the game's own
  remote can't serve, and every CI checkout spends GitHub's LFS bandwidth.
- **Moving examples out in this change.** A bigger change touching the book,
  the play site and every example PR, for the issue's whole budget at once;
  decided as the direction instead (decision 1).

## Consequences

- An agent adding a texture over 2048 texels, a second copy of a file, or a
  file over 8 MB gets told by `scripts/ci.sh`, with the fix (the tool, or
  pointing at the one copy).
- Editing a borrowed example file fails CI until the fixture keeps its old
  bytes. The message says how.
- Twelve of 116 golden renders moved, by at most 30 levels in one pixel and
  0.03 on average (the selection outline's planks, the Lantern and bench in
  the meadow scenes); the moved goldens were re-blessed with the change.
- `testModels/` is gone. Its one unique file, `DoubleSidedCard.glb`, is
  `tests/fixtures/models/`; the other tests read the examples' models (until #984 gave the fixtures their own).
- When examples move out, `borrowed.toml` lists exactly which bytes the
  fixtures must take back.

## Measured

| | Before (main, 2026-11-14) | After |
|---|---|---|
| Tracked tree (git's blob sizes) | 390.4 MB, 2,195 files | 168.3 MB, 2,149 files |
| Bytes in duplicate files, past the first copy | 117.3 MB | 1.8 MB (11 named groups) |
| A game's working tree, full | 378 MB | 166 MB |
| A game's working tree, sparse recipe | 76 MB | 41 MB |
| A game's `.git` after `subtree add --squash` and `gc` | 210 MB | 123 MB |
| The book's images | 85.6 MB | 67.5 MB (same pixels) |
| Wooden bench / planks / Lantern | 32.1 / 27.7 / 9.6 MB | 3.6 / 1.9 / 6.2 MB |

"Before" is main at the merge (#760 had just added the showcase's sounds);
the Context's 390 MB and 115 MB are the same tree a few commits earlier.
`size-check.py` measures files on disk, so its 168.8 MB counts a Windows
checkout's line endings. The game numbers are a throwaway repository with the engine added as a
squashed subtree from this checkout, before and after.

### The first breach (#1276, 2026-11-18)

The book crossed 78 MB with Understudy (#885: a 3.8 MB film and six stills)
and was 78.8 MB on main a day later. 91 of its 205 PNGs were RGBA with
every pixel opaque, renders saved since #759. `pack_book_images.py` packed
124 of them (alpha dropped, Pillow's slowest zlib): 70.35 to 66.64 MB of
PNGs, every pixel checked against the old file, and the book to 75.1 MB.
The budget stayed. The film stayed too: it was encoded tightly already
(crf 28 saved 0.5 MB; crf 30 saved 1.3 MB at SSIM 0.984 against a copy
that is itself lossy, the scene being in another repository), so the
lossless repack was the cheaper room.

### The examples leave (#984, 2026-11-20)

Decision 1's split, made: the examples are the flint-examples repository
([ADR 0131](0131-the-examples-are-a-project-of-their-own.md)), and
`borrowed.toml` listed exactly what the fixtures took back, 26 files,
19.2 MB, the same bytes. The showcase game kept its own copy of the
bartender's YBot (2.4 MB, a named duplicate of the fixtures' copy). The
showcase game's ten sounds and Sun Prints' palm stopped being duplicates.
Measured by `size-check.py --report` on this Windows checkout (files on
disk), main just before against the branch:

| | Before (main 7b7f485d) | After |
|---|---|---|
| A game's `engine/` (the tracked tree) | 181.4 MB, 2,563 files | 158.0 MB, 2,176 files |
| the examples and the engine proper | 66.0 MB together | 20.4 MB, the engine alone (still over its 20 MB, #1303) |
| tests | 25.9 MB | 45.7 MB |
| `showcase_game/` | 13.8 MB | 16.2 MB |
| Named duplicate groups | 16 | 6 |

The whole-tree budget goes to 175 MB (the measurement and about a tenth),
tests to 50 MB, `showcase_game/` to 18 MB. The engine area's breach is
#1303's and is left for it. The sparse recipe no longer names
`engine/examples`.

### The engine area grows by its code (#1303, 2026-11-20)

The engine area crossed 20 MB as code, not as a file to shrink. On main
04a9588b it is 20.5 MB on this Windows checkout (files on disk, CRLF) and
20.1 MB on CI's Linux checkout; git's blob sizes total 19.06 MiB, of which
`.rs` files are 16.42 MiB (86%), Markdown 0.91 (the month's changelog alone
0.31), WGSL 0.51, TOML 0.31, and every image together 0.22 (the play site's
posters). Its largest files are `changelog/2026-11.md`, `stylize.rs`'s GPU
tests, `Cargo.lock` and the texture ops. In the two days from #1303's
measurement (19.98 MB) to here it grew about 0.5 MB, spread over a dozen
crates (render +169 KB, cli +116, player +109, script +58, physics +48,
procgen +44), the largest single addition `frame_grab.rs` at 38 KB.

Source is what this area is for, and this budget can't be met by moving or
re-encoding anything, unlike the book's in #1276. So the budget moves, by
the decision's own rule: the measurement and about a tenth, **23 MB**. The
whole-tree budget (175 MB, 158.2 measured) has room for it. At the rate
measured here that is a week or two of work, so the next breach is expected
soon; when it comes, the question to argue is whether source code needs
a byte budget at all, rather than another tenth.
