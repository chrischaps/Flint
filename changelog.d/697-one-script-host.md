### `flint render --scripts` does what the player does with styles, looks and the camera; `ScriptSystem`'s `take_*` methods are gone (#697)
**What changed.** Script commands go through one `ScriptHost` interface that
the player and `flint render --scripts` both implement. A capture now applies
`set_entity_style`, `set_style_param`, `tween_style_param`,
`clear_entity_style`, the whole-frame styles, `set_look`, `set_camera_roll`,
`set_render_mode`, `set_lod_scale`, `set_quality`, `reset_style_history`,
`clear_living` and the state functions, and `get_entity_style` reads there;
what it can't do (sound, the cursor, scene changes) is
`flint_player::PRESENTATION_ONLY`. The camera, post-processing, time scale,
low-pass, touch, LOD, cursor and quality requests arrive from
`ScriptSystem::drain_commands` as `ScriptCommand`s (`SetCamera`, ...);
`take_camera_overrides` and the nine other `take_*` methods are removed, and
`flint_player`'s `apply_style_command` / `apply_post_style_command` became
methods per command.
**What to do.** Nothing in a script. A game binary that called a `take_*`
method or matched `ScriptCommand` itself implements `flint_script::ScriptHost`
and calls `flint_script::host::apply_all`. Captures of scenes using these
functions change to show them.
