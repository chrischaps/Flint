### Tracy (and the scope recorder) see inside physics, scripts, animation, loading and the renderer (#1096)
**What changed.** About 120 `flint_core::scope!` lines now sit below
the frame loop's sixteen: physics (`physics::fixed_update`, its step and both
syncs, the character), scripts (each callback kind, each script's call by
file, the hot-reload poll, compiles), animation (each tier, IK), particles,
audio, the procgen queue and living props, terrain, loading
(`import_model`, `decode_image`, `load_texture`, `upload_model`,
`scene_transition`, each by file or asset name) and the renderer's inner
steps (extraction, culls, uniforms, each post effect and style part as
`encode_part`, pipeline builds by label, egui's run, tessellation and
upload). Loops carry one scope with a count, never one an entity. A
`&Path` is now a scope detail (`scope!("load_level", path)`), read only
while the scope records. A default release build still carries none of it.
**What to do.** Nothing.
