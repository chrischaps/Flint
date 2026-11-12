### `PlayerApp`'s launch and scene fields are grouped: `app.launch.*`, `app.scene.*` (#724)
**What changed.** A game binary that builds a `PlayerApp` itself sets its
options through two groups. What `flint-player`'s flags set is
`app.launch` (`LaunchOptions`): `msaa_sample_count`, `window_size`,
`window_options`, `render_scale`, `quality_overrides`, `style_specs`,
`style_time`, `look`, `hold_at`, `save_dir`, `expression_feed`, `fullscreen`
and the rest. What the scene's file says is `app.scene` (`SceneSettings`),
with the `scene_` prefix dropped: `skybox_path`, `ambient`, `camera`,
`post_process`, `tables`, `preload_audio`, `input_config`, ... The `[living]`
and `[wooze]` tables are no longer public fields; read them with
`ExtensionCtx::scene_table`. Nothing a scene or script does changes.
**What to do.** In a game's `main.rs`, `app.msaa_sample_count = 4` becomes
`app.launch.msaa_sample_count = 4`, and `app.scene_camera = ...` becomes
`app.scene.camera = ...`; better, call `app.apply_scene_settings(&scene_file)`,
which sets every `app.scene` field from the file.
