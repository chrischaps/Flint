# Frozen scripts (#984)

Copies of flint-examples' scripts that flint-script's unit tests read, taken
on 2026-11-20 when the examples moved to their own repository
([flint-examples](https://github.com/chrischaps/flint-examples)):

| File | Read by |
|---|---|
| `showcase_controller.rhai`, `ui/pause.ui.toml`, `ui/pause.style.toml` | `crates/flint-script/src/engine/ui_tests.rs`: the showcase's pause menu, driven by keys and a tap |
| `showcase_hud.rhai` | `crates/flint-script/src/engine/tests.rs`: a real script that ends its own fades |
| `audio_clock.rhai` | `cargo run -p flint-script --example top_level_rerun`: a script with a dozen top-level lets |
| `seeded_stars.rhai` | `cargo run --release -p flint-script --example api_overhead`: the heaviest script-drawn scene |

Three paths changed from the originals, so the copies stand alone: the
controller loads `ui/pause.ui.toml` (it named the file from the
engine checkout's root), the menu names `ui/pause.style.toml`, and the HUD
compares `get_current_scene()` with `showcase_crucible.scene.toml`. Nothing
else was edited. They live outside `tests/fixtures/scenes/` because no scene
here loads them, so `flint check` on the scenes doesn't read them.
