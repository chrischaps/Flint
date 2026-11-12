### `PlayerExtension` hears the frame's stages and the scene, and brings its own script module (#754)
**What changed.** A game's extension gets the hooks the engine's own
subsystems have: `update(stage, ctx)` at each `flint_player::Stage` of the
frame (after the engine's subsystems), `scene_loaded(ctx)` and
`scene_unloading(ctx)`, `script_command(command, ctx)` to take a
command before the engine applies it, and `script_modules()` to declare a
script module of its own, enabled for the whole run.
`ExtensionCtx::set_module_state(value)` hands that module what its
functions read. Every new method defaults to nothing, so an extension
compiles as it did. They run only in the player: `flint render --scripts`
links no game's Rust.
**What to do.** Nothing. A game whose Rust registered raw functions can move
them into a module of its own (the player-extensions guide shows one), and
work that belongs after physics or after the scripts' UI can move from
`pre_update` to `update` at that stage.
