# Frozen test scenes (#763)

The scenes the stylize goldens (`crates/flint-cli/tests/golden/stylize.goldens.toml`)
and the CLI tests load, with the text assets they read: the props prefab,
user styles, fonts, palettes and particle effects. The binaries they share
with the examples (models, textures, papers, strokes, LUTs, terrain) they
**borrow**: each is named by its path in `examples/`, and pinned (#759).

The scenes are copies. Most started as scenes in `examples/`, frozen here on
2026-11-10 so the examples can be improved without re-blessing a golden.
They render byte-identical to the `demo/` scenes they were copied from (all
79 cases, checked against a control run of main; `style-veil` flips between
two frames on main too, and this copy draws one of them).

- **Don't edit one to improve it.** A change here moves goldens. Make it on
  purpose, in the same commit as the re-bless, and say why.
- **Don't point a golden or a test at `examples/`.** `render_goldens.rs`'s
  `no_golden_case_loads_an_example` fails the build when a case does. Copy
  the scene here and point at the copy.
- **Borrow an example's binary; don't copy it** (#759, ADR 0122). A copy is
  the same bytes tracked twice, which `scripts/size-check.py` refuses. Name
  the file by path from the scene (`asset = "../../../examples/models/YBot.glb"`,
  `texture = "../../../examples/textures/wood_planks.jpg"`; from `styles/`
  one more `../`) and pin it in `../borrowed.toml` with its SHA-256. The
  check fails when a pinned file changes, so improving a borrowed example
  file is copy on write: first copy the old bytes here
  (`git show HEAD:examples/... > <folder>/...`), point the fixture at the
  copy and drop the pin, then change the example. A binary only the fixtures
  use lives here, as `../models/DoubleSidedCard.glb` does.
- **They are written in the examples' archetypes** (`furniture`), which are the layer `schemas/examples`, not the engine's set (#681). The repository's `flint.toml` lists it, so a fixture run with no `--schemas` resolves them; a flag is the whole list, so the suite and the tests pass `--schemas schemas --schemas schemas/examples`.
- **Most have no light, on purpose.** They are lit by the engine's built-in key and fill (`default_scene_lights`), and the goldens pin that. They used to spell a sun as `[entities.sun.directional_light]`, a table nothing reads; #796 deleted those, which moved no golden. The examples they were copied from carry real `light` tables (#681); giving these one would move every golden that loads them, so do it only to re-bless on purpose. A light is `[entities.<name>.light]` with a `type` (`pencil_meadow` has some).
- **They pass `flint check`** (#796, `crates/flint-cli/tests/fixture_scenes_cli.rs`): a fixture with an error diagnostic fails that test, unless its `KEPT` names it with the reason (now `styles/darkroom.shader.toml`, #1056).
- **Assets are found by the one asset search** (ADR 0114): a scene here
  looks in `tests/fixtures/scenes/<folder>/`, then up to the repo root,
  never in `examples/`. A fixture that renders with something missing
  usually warns; `flint asset where <kind> <name> --scene <scene>` shows
  where it looked.

| Scene | Loaded by |
|---|---|
| `stylize_props_plain`, `_ascii`, `_pencil`, `stylize_props`, `stylize_blend` | the props goldens: one bench per style (#386) |
| `stylize_props_scripted` (+ `scripts/story_beat.rhai`) | the scripted-render goldens, `script-beat*` (#371): written here, not copied from an example |
| `pbr_test` | ASCII and look goldens; `render_goldens.rs`'s flag test |
| `ascii_edges`, `ascii_entities`, `selection_outline`, `pencil_sketch`, `watercolor`, `user_styles`, `style_terrain`, `style_veil` | their own goldens; `user_styles` also `list_styles_cli.rs` |
| `mesh_sequence` | `render_mesh_cli.rs` |
| `ascii_motion` | the sequence goldens `ascii-motion-tone` and `ascii-motion-edges` (#478): `examples/ascii_motion` on 2026-11-13 without its orbit script, which `orbit = 6.8755` replaces. A change here moves their counts: re-measure and say so beside the budgets |
| `pencil_meadow` | `look-rotoscope-meadow*` (#883): `examples/reveal_tear`'s clearing and meadow as a still, written here with real `light` tables |
| `pencil_dark_sky` | `look-rotoscope-dark-sky*` (#890): written here, a YBot under the default dark sky with real lights |
| `pencil_rolling` | `pencil-local` (#544): `examples/pencil_rolling` on 2026-11-17 without its script, the casks posed mid-roll and the crate turned; real `light` tables |
| `style_veil_echo`, `style_veil_billboard`, `style_mask_wooze` | `style-veil-echo`, `style-veil-billboard`, `style-mask-wooze[-msaa4]` (#737): written here with real lights. The echo scene's oak is `specs/examples/oak_living.procgen.toml`, found by the asset search from the repo root, so a change to that spec moves `style-veil-echo` |
