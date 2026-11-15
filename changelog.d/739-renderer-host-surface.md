### `SceneRenderer`'s per-frame fields and setters are private: `bind_scene` and `prepare` replace them (#739)
**What changed.** A host that embeds `flint_render::SceneRenderer` drives it through `bind_scene` (once a scene) and `prepare(device, queue, &world, &FrameInputs)` (once a frame) only. What each removed name became:
- `ocean_time` → `FrameInputs::time`, or `look_time` to hold it (`--style-time`).
- `grass_time`, `grass_wooze` → `grass_time`, `grass_wooze`; `style_dt` → `dt`.
- `pixels_per_point` → `pixels_per_point` (read back with `pixels_per_point()`).
- `camera_offset`, `ortho_height`, `aspect_ratio` → `view_2d: View2d` (`View2d::of(&camera)`).
- `set_boil` → `boil`; `set_shallow_wooze` → `shallow_wooze`; `set_world_lines` → `world_lines`.
- `update_particles_from` → `particles`; `update_grass_entities` → `grass_benders`.
- `update_from_world` → `prepare`.
- `scene_dir`, `set_asset_scope`, `set_terrain_entity` → `bind_scene` (`scene_dir()` reads it).

`update_particles` (a host's own draws, after `prepare`) and `set_motion_time` (before bone writes) stay. Pipeline and GPU-uniform types are crate-private; `flint_render::MAX_BONES` and `WORLD_LINE_DEPTH_SLACK` replace the `skinned_pipeline::` and `world_line_pipeline::` paths.
**What to do.** Nothing for a game on the engine's binaries. Rust that drove the renderer by hand: build one `FrameInputs` a frame, as `crates/flint-render/CLAUDE.md` ("Host protocol") shows.
