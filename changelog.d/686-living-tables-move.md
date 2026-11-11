### Living props' scene types moved from `flint_scene` to `flint_procgen` (#686)
**What changed.** For a game with its own Rust binary only; scenes and
scripts are untouched. `LivingDef`, `LivingSpeciesDef`, `LivingEchoesDef`,
`LivingVarianceDef`, `WoozeDef`, `WoozePreset`, `WoozeKnobs`, `BoilDef`,
`WoozeIssue`, the preset functions (`read_preset`, `write_preset`,
`list_presets`, `apply_preset_file`) and `commit_look` / `look_edits` are
`flint_procgen::` now. `SceneFile` has no `living` or `wooze` field: the two
tables are in `scene_file.tables`, read with
`flint_procgen::LivingTables::of(&scene_file)`. `flint_scene::load_scene` no
longer fills in the file a `[wooze] preset` names.
**What to do.** Change the paths. After `load_scene`, call
`flint_procgen::settle_scene(&mut scene_file, path)?`; a binary that calls
`PlayerApp::apply_scene_settings` gets the preset filled in without it. A
misspelled key in `[living]` or `[wooze]` still refuses the scene, now naming
the table instead of a line number.
