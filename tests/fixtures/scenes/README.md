# Frozen test scenes (#763)

The scenes the stylize goldens (`crates/flint-cli/tests/golden/stylize.goldens.toml`)
and the CLI tests load, with the text assets they read: the props prefab,
user styles, fonts, palettes and particle effects, and the binaries they
draw (models, textures, papers, strokes, LUTs, terrain). From #759 to #984
they borrowed those binaries from the examples by path, pinned by SHA-256;
when the examples moved to their own repository
([flint-examples](https://github.com/chrischaps/flint-examples), #984) the
fixtures took back the 26 files they borrowed, same bytes.

The scenes are copies. Most started as scenes in the examples, frozen here on
2026-11-10 so the examples can be improved without re-blessing a golden.
They render byte-identical to the `demo/` scenes they were copied from (all
79 cases, checked against a control run of main; `style-veil` flips between
two frames on main too, and this copy draws one of them).

- **Don't edit one to improve it.** A change here moves goldens. Make it on
  purpose, in the same commit as the re-bless, and say why.
- **A test needs its scene here.** The examples are another repository
  (#984), so no test can load one; `render_goldens.rs`'s
  `no_golden_case_loads_an_example` still refuses a case that names one.
  Copy the scene and what it reads here (from a `flint-examples` clone) and
  point at the copy.
- **A binary is named from the scene, as written**
  (`asset = "./models/YBot.glb"`, `texture = "./textures/wood_planks.jpg"`;
  from `styles/` it is `../`), so the asset search takes the path as it is.
  The same bytes twice in this tree is a duplicate `scripts/size-check.py`
  refuses: point the second user at the first.
- **They are written in the examples' archetypes** (`furniture`), which are the layer `schemas/examples`, not the engine's set (#681). The repository's `flint.toml` lists it, so a fixture run with no `--schemas` resolves them; a flag is the whole list, so the suite and the tests pass `--schemas schemas --schemas schemas/examples`.
- **Most have no light, on purpose.** They are lit by the engine's built-in key and fill (`default_scene_lights`), and the goldens pin that. They used to spell a sun as `[entities.sun.directional_light]`, a table nothing reads; #796 deleted those, which moved no golden. The examples they were copied from carry real `light` tables (#681); giving these one would move every golden that loads them, so do it only to re-bless on purpose. A light is `[entities.<name>.light]` with a `type` (`pencil_meadow` has some).
- **They pass `flint check`** (#796, `crates/flint-cli/tests/fixture_scenes_cli.rs`): a fixture with an error diagnostic fails that test, unless its `KEPT` names it with the reason (only the goldens' broken styles now; `styles/darkroom.shader.toml` was kept until #984 gave it its own images, and the golden `user-style-darkroom` draws it since #1056).
- **Assets are found by the one asset search** (ADR 0114): a scene here
  looks in `tests/fixtures/scenes/<folder>/`, then up to the repo root. A fixture that renders with something missing
  usually warns; `flint asset where <kind> <name> --scene <scene>` shows
  where it looked.

| Scene | Loaded by |
|---|---|
| `stylize_props_plain`, `_ascii`, `_pencil`, `stylize_props`, `stylize_blend` | the props goldens: one bench per style (#386) |
| `stylize_props_scripted` (+ `scripts/story_beat.rhai`) | the scripted-render goldens, `script-beat*` (#371): written here, not copied from an example |
| `pbr_test` | ASCII and look goldens; `render_goldens.rs`'s flag test |
| `ascii_edges`, `ascii_entities`, `selection_outline`, `pencil_sketch`, `watercolor`, `user_styles`, `style_terrain`, `style_veil` | their own goldens; `user_styles` also `list_styles_cli.rs` |
| `mesh_sequence` | `render_mesh_cli.rs` |
| `ascii_motion` | the sequence goldens `ascii-motion-tone` and `ascii-motion-edges` (#478): flint-examples' `ascii_motion` on 2026-11-13 without its orbit script, which `orbit = 6.8755` replaces. A change here moves their counts: re-measure and say so beside the budgets |
| `pencil_meadow` | `look-rotoscope-meadow*` (#883): flint-examples' `reveal_tear`'s clearing and meadow as a still, written here with real `light` tables |
| `pencil_dark_sky` | `look-rotoscope-dark-sky*` (#890): written here, a YBot under the default dark sky with real lights |
| `pencil_rolling` | `pencil-local` (#544): flint-examples' `pencil_rolling` on 2026-11-17 without its script, the casks posed mid-roll and the crate turned; real `light` tables |
| `tron_ground` | `reality-tron-ground` (#1181): written here, a 500 m tiled plane seen from 2 m at 5 degrees down and one crate; no light, so the built-in pair |
| `style_veil_echo`, `style_veil_billboard`, `style_mask_wooze` | `style-veil-echo`, `style-veil-billboard`, `style-mask-wooze[-msaa4]` (#737): written here with real lights. The echo scene's oak is `specs/examples/oak_living.procgen.toml`, found by the asset search from the repo root, so a change to that spec moves `style-veil-echo` |
| `style_veil_wooze` | `style-veil-wooze` (#1241): a drifting pane veils a glyph crate where it is drawn; a still pane beside it is the control. Real light |
