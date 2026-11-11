# Changelog for games built on Flint

This file is for a game that carries the engine as `engine/` and has just
pulled a newer one (`git subtree pull --prefix=engine ... --squash`). The
squash hides the engine's commit subjects, so this is where to read what
changed and whether your game has to do anything.

It lists what a game can notice: scene and schema keys, the script API, CLI
flags, file formats, Cargo features, and changes in how something looks,
sounds or behaves. Engine internals, new demos and tooling for the engine
itself are left out. Newest first. Each entry says **what changed** and, where
there is something, **what to do**; "nothing" means old files and scripts keep
working.

## Adding an entry

A pull request that changes `schemas/`, the script API
(`crates/flint-script/src/api.rs`) or the scene format
(`crates/flint-scene/src/format.rs`) needs an entry here, and CI's `changelog`
job fails without one (`scripts/changelog-check.sh`). If the change is one no
game can notice, say so in a commit message on the branch instead, on a line
of its own:

    Changelog: none (reason)

Changes to CLI flags, Cargo features, file formats and defaults aren't
detected by the check. They still belong here.

Write the entry for someone who has never read the PR: name the key, function
or flag as a game author types it, say what happens to old files, and say what
to do.

---

## 2026-11-10

### `flint.toml`'s `schemas` is read by every command; `--schemas` is the whole list (#706)
**What changed.** One rule now decides which schema directories load, the same
in `flint play`, `flint-player`, `flint render`, `flint validate`, `flint
query`, `flint entity`, `flint schema`, `flint edit` (the scene viewer and
`--spline`) and `flint prefab view`:

1. the `--schemas` flags, if any were given;
2. else `[project] schemas` in the `flint.toml` of the project the scene is
   in (the working directory's project for `flint schema`);
3. else, as before, `schemas/` in the working directory, beside the scene and
   one level up, whichever exist.

So a game whose `flint.toml` says `schemas = ["engine/schemas", "schemas"]`
needs no schema flags on any command, from any folder of the project. What a
game can notice:

- **`flint query` and `flint entity` take `--schemas`**, repeatable, and read
  the manifest. `query` used to load only `schemas/` in the working directory
  (and hid a schema file that didn't parse: that is an error now); `entity`
  took one directory.
- **`flint edit <scene>` loads every `--schemas`**, and constraints from each.
  It used to take the first and drop the rest, so a game's scene opened with
  the engine's archetypes and none of its own.
- **`--schemas` is the whole list.** `play`, `render`, `validate`, the spline
  editor and `flint-player` used to add any `schemas/` beside the scene or one
  level up to the flags; with flags they no longer do. A game run from its
  root with `--schemas engine/schemas --schemas schemas` loads what it did.
  For this release, those commands print a line on stderr for each directory
  they would have added and now leave out: `warning: schemas is no longer
  added to --schemas: the flags are the whole list now. Pass it with another
  --schemas, or list it under [project] schemas in flint.toml`. The warning
  goes away in a later release; the behaviour stays.
- **A named directory that doesn't exist is an error** in every command,
  where it used to be skipped: `schema directory not found: nope (named by
  --schemas)`, or `(named by `schemas` in <root>/flint.toml)`. This includes a
  manifest listing `engine/schemas` in a checkout with no engine subtree.
- With no flags and no `schemas` key, each command finds what it found
  before. In the player, a scene transition into another project reads that
  project's manifest.
- `flint bundle` uses the manifest's `schemas` when `bundle.toml` has no
  `schemas` key (it used to assume `engine/schemas` and `schemas`).
- For a game with its own binary: `PlayerApp::set_schema_paths` works as
  before; `set_schema_dirs` takes the answer of
  `flint_schema::SchemaRegistry::resolve(scene, flags)`, which is how to get
  the same directories the commands do. `flint_schema::discover_schema_dirs`
  is deprecated.

**What to do.** Add `schemas = ["engine/schemas", "schemas"]` under
`[project]` in your `flint.toml` and drop the flags from your scripts, or keep
the flags: both load the same thing. If a command now stops with `schema
directory not found`, the flag or manifest entry it names was being ignored
before: fix the path or remove it. If you relied on a `schemas/` folder beside
a scene being added to your flags, name it with another `--schemas`.

### `random()` no longer repeats, and scripts can `seed_random(n)` (#580)
**What changed.** `random()` and `random_range(min, max)` used to hash the
wall clock on every call, so calls close together (two in a row in one
`on_update`, say) often returned the same number. They now draw from one
sequence shared by every script. It starts from the clock once, when the
player starts, so plain runs still differ from each other, but no two calls
repeat. The new `seed_random(n)` restarts that sequence at `n` and gives
the numbers `--seed n` gives, every run. It reseeds every script's draws,
not only the caller's. `--seed`, `--frame-step` and `flint render --scripts`
are unchanged.
**What to do.** Nothing. A script that wants a replayable run calls
`seed_random(n)` in `on_init`, not at the top of the file: top-level
statements run again before every callback, so a seed there restarts the
sequence every frame.

### `flint.toml` gains a `[project]` table, and a bad one is refused (#706)
**What changed.** The `flint.toml` at a project's root, until now only a
marker, may hold `[project]` with `id` (a string), `schemas` (a list of
folders relative to the root, in override order), `scene` and `input` (paths
relative to the root). Every key is optional, an empty file is still a valid
marker, and other tables are left alone. `schemas` is read (the entry above);
`id`, `scene` and `input` are parsed and checked and not yet acted on: the
scene argument and the input config work as before. The commands that look for a project root (`flint
asset ...`, `flint gen --register`, anything given `--project <dir>`) now stop
with an error naming the file and the key when `[project]` holds a value of
the wrong type or a key Flint doesn't know.
**What to do.** Nothing, unless your `flint.toml` already has a `[project]`
table with other keys in it: move those to a table of another name. You can
add `id` and `schemas = ["engine/schemas", "schemas"]` now (ADR 0114).

### Escape reaches the game when the game binds it (#639)
**What changed.** If any action in the loaded input config binds Escape, a
press no longer closes the window: it goes to the game like any other key.
With nothing bound to Escape, it quits as before. Alt+F4 and the close button
still quit.
**What to do.** A game that binds Escape (to a pause menu, say) must offer its
own way out. `quit_game()` (#659) ends the run as closing the window does.

### A scripted run with a missing screenshot fails (#639)
**What changed.** `flint-player` and `flint play` exit 1 when a requested
screenshot was never written, when a `--screenshot-every` sequence lost
frames, or when the player failed to start. They used to exit 0.
**What to do.** Nothing, unless a capture script relied on exit 0. A run that
now fails was already missing its output.

### Entity setters warn on a missing entity (#635)
**What changed.** `set_field`, `set_position`, `set_rotation`,
`set_rotation_quat`, `set_scale`, `set_shear`, `clear_shear`, `rotate_local`,
`set_visible`, `set_joint_target`, `set_parent`, `set_material_color` and
`set_material_override` warn, once per script and function, when given `-1`
(what `get_entity` returns for a name it can't find) or a despawned id. They
used to do nothing in silence.
**What to do.** Read the new warnings: each is a script acting on an entity
that isn't there.

### `--frame-step` runs draw the same random numbers every time (#635)
**What changed.** Under `flint-player --frame-step`, scripts' `random()` and
`random_range()` are seeded by default, so two stepped runs are identical.
`--seed N` (and the web page's `?seed=`) picks another sequence. A normal,
unstepped run is unchanged.
**What to do.** Nothing. For variety in a stepped capture, pass `--seed`.

### OGG loops play at their true length (#632)
**What changed.** OGG Vorbis files are decoded gapless: the padding an encoder
adds at the end is dropped, so a loop is exactly as long as its audio. Before,
each pass of a loop carried a short gap. WAV is unchanged.
**What to do.** If a loop was edited to hide the old gap, or music was timed
against the padded length, re-check it.

### Sounds can be named by their stem (#629)
**What changed.** `play_sound("whoosh")`, `play_sound_at` and `audio_source`
names find `audio/whoosh.ogg` (or `.wav`, `.flac`, `.mp3`). The full
`audio/whoosh.ogg` still works and wins when both could match. A miss now
names the nearest sound.
**What to do.** Nothing.

### Contour ink is a style; `kuwahara_boil` and `contour_*` have new homes (#599)
**What changed.** Contour settings live in `[post_process.styles.contour]`
(`width`, `threshold`, `strength`, `luma`, `color`, `fade`, `boil`), and
Kuwahara's boil is `boil` in `[post_process.styles.kuwahara]`. The old flat
keys (`contour_enabled`, `contour_*`, `kuwahara_boil`) still load and are
folded into those tables. A plain `flint render --contour` now draws its ink;
on the previous engine it drew none.
**What to do.** Nothing. Prefer the tables in new scenes.

### A sun of intensity 0 casts no shadows; scenes can switch lighting off (#627)
**What changed.** A directional light with `intensity = 0` no longer runs the
shadow pass (the picture is the same; the cost is gone). Three new keys:
`[environment] shadows = false`, `[environment] default_lights = false` (a
scene with no lights is lit by its ambient alone, not the built-in key and
fill), and `material.unlit = true`.
**What to do.** Nothing. A scene that kept a dummy zero-intensity sun only to
keep the built-in lights away can use `default_lights = false`.

### glTF `doubleSided` materials draw from both sides (#623)
**What changed.** A glTF material marked `doubleSided` is drawn, lit and
shadowed from both faces. The flag used to be ignored and back faces were
culled.
**What to do.** Check models whose export set `doubleSided` by accident: their
back faces now show and cost fill. Clear the flag in the asset.

### New for scripts
`audio_position(entity)` (#658), `quit_game()` (#659), `safe_area()` and
`is_touch_device()` (#660), `set_scale` and `set_visible` (#628), and a
`shear` on `transform` with `set_shear` / `clear_shear` (#625).

## 2026-11-09

### Sound, gamepads and file dialogs are Cargo features (#221)
**What changed.** `flint-player` has `audio` and `gamepad` features and
`flint-cli` has `audio`, `gamepad` and `dialogs`. All are on by default. The
workspace's device crates are declared with `default-features = false`.
**What to do.** A game binary that depends on `flint-player` with
`default-features = false` must name `audio` and `gamepad` (and `debug-hud`)
to keep sound, pads and the debug panels. With default features, nothing.

### A GLB with `LOD_0`, `LOD_1`... root nodes draws one level at a time (#193)
**What changed.** The importer recognises a chain of root mesh nodes named
`LOD_<n>` and switches between them by distance. Before, every level drew at
once. A chain that carries its own `extras.flint_lod` bands is left alone.
**What to do.** Nothing for trees made by `flint gen` with `[[lod]]` tables:
they now look right. A hand-made model whose nodes are named `LOD_<n>` but are
meant to draw together needs other names.

### Sprite text draws through the glyph atlas (#283, ADR 0113)
**What changed.** `ui_text.font` can name `builtin`, a glyph set (a
`.glyphs.toml`, so a TTF too) or a `.font.toml`. A `.font.toml` still works.
Its glyphs no longer pick up a sliver of the neighbouring cell when filtered.
**What to do.** Nothing. Text from a `.font.toml` may look very slightly
cleaner at its edges.

### A glyph ramp that steps backwards in ink is reported (#281)
**What changed.** A project glyph set whose ramp gives a brighter step less
ink than a darker one logs the offending pairs. `map = "coverage"` in
`[ramps.<name>]` re-orders a ramp by measured ink.
**What to do.** Nothing. Add `map = "coverage"` if the warning names your ramp
and the banding bothers you.

### New
`flint export web <game>` packs any game as a folder a static server plays,
and `bundle.toml` gains `[web]` and `[mount]` (#106). `flint play` and
`flint-player` take `--width`, `--height`, `--position`, `--borderless`,
`--always-on-top` and `--no-activate` (#613).

## 2026-11-08

### New
`[post_process] look = "<name>"` in a scene, with `set_look` / `clear_look`
for scripts (#356). `set_post_style`, `set_post_style_param` and
`reset_post_style` for scripts, and a `blend` key on every style's table
(#369). User-authored styles gain images, options, more than one pass, and
per-entity use (#412, #413, #414).

## 2026-11-07

### `--style` refuses a number outside its range (#355)
**What changed.** `flint render --style ascii:cell=400` (and `--style` on
`flint play`, `flint-player` and `flint edit`) fails with the allowed range. A
scene table with an out-of-range value warns and keeps the value.
**What to do.** Fix the value the error or warning names.

### Unknown `[post_process]` keys warn (#354)
**What changed.** A key in a scene's `[post_process]` that the engine doesn't
know is reported with the closest real name. It used to be ignored.
**What to do.** Read the warning: it is usually a typo that had been doing
nothing.

### Kuwahara's settings live in `[post_process.styles.kuwahara]` (#298)
**What changed.** `radius` (whole pixels, 1 to 8), `sharpness`, `hardness` and
`anisotropy` are keys of the style's table. The old flat `kuwahara_*` keys
still load and are folded into it; a table made that way is on only with
`kuwahara_enabled = true`, as before.
**What to do.** Nothing. Prefer the table in new scenes.

### A style colour is one key (#315)
**What changed.** `tint = [1.0, 0.7, 0.25]` or `tint = "#ffb340"` in a style's
table or a per-entity style component. On the command line,
`--style ascii:tint=#ffb340` or `tint=1/0.7/0.25`. The single channel keys
(`tint_r`, `tint_g`, `tint_b`) still read.
**What to do.** Nothing.

### The ASCII spike's flags are gone (#293)
**What changed.** `flint render --ascii=full|entity|entity-object`,
`--ascii-entities` and `--ascii-scene-background` were removed. `--ascii` is a
plain switch for the whole frame.
**What to do.** For ASCII on some entities only, give them an `ascii_style`
component (or a `style_mask`), or pass `--ascii-scope entities`; for the
object-anchored grid, `grid = "object"` or `--ascii-grid object`.
