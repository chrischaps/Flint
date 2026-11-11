### Input, time, audio, animation, physics, camera, post-process, math, event and log functions take integers; `log(42)` logs (#695)
**What changed.** These groups are now declared like the Entity API: a number
parameter takes an int (`set_camera_fov(60)`, `sin(1)`, `play_sound("hit", 1)`),
an int parameter takes a whole float (`is_mouse_pressed(0.0)`), and
`play_clip`, the animation layers, `audio_position`, `set_velocity_2d` and
`get_velocity_2d` warn once for `-1` or a despawned id and then do nothing
(`set_velocity_2d` no longer queues one). `log`, `log_warn`, `log_error` and
`log_info` take any value. `log(x)` with one float used to be Rhai's base-10
logarithm, silently; it now logs the number. `flint script api` opens with
these rules (`conventions` in `--json`).
**What to do.** If a script used `log(x)` as a logarithm, write `log(x, 10.0)`.
A new warning names a call on a missing entity: check it with
`entity_exists(id)` first.
