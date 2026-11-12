# Frozen test scenes (#763)

The scenes the stylize goldens (`crates/flint-cli/tests/golden/stylize.goldens.toml`)
and the CLI tests load, with every asset they read: models, textures, the
props prefab, user styles, papers, strokes, fonts, palettes, terrain and
particle effects.

They are copies. Most started as scenes in `examples/`, frozen here on
2026-11-10 so the examples can be improved without re-blessing a golden.
They render byte-identical to the `demo/` scenes they were copied from (all
79 cases, checked against a control run of main; `style-veil` flips between
two frames on main too, and this copy draws one of them).

- **Don't edit one to improve it.** A change here moves goldens. Make it on
  purpose, in the same commit as the re-bless, and say why.
- **Don't point a golden or a test at `examples/`.** `render_goldens.rs`'s
  `no_golden_case_loads_an_example` fails the build when a case does. Copy
  the scene here, with the assets it loads, and point at the copy.
- **They are written in the examples' archetypes** (`furniture`), which are the layer `schemas/examples`, not the engine's set (#681). The repository's `flint.toml` lists it, so a fixture run with no `--schemas` resolves them; a flag is the whole list, so the suite and the tests pass `--schemas schemas --schemas schemas/examples`.
- **Their lights are tables nothing reads, on purpose.** Eight of them (and the golden suite's `broken_style` fixture) spell a sun as `[entities.sun.directional_light]`, so they are lit by the engine's built-in key and fill, and the goldens pin that. The examples they were copied from now carry real `light` tables (#681); making these real would move every golden that loads them, so they stay until someone re-blesses on purpose. #683's unknown-component diagnostic will name them.
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
