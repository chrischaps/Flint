### `flint_core::scope!` marks a scope for Tracy and Flint's recorder; a `trace` feature (#1095)
**What changed.** The engine's profiler scopes are `flint_core::scope!("name")`
lines (ADR 0128), with an optional count or text detail
(`scope!("load_level", path)`). One line reaches Tracy under the player's
`profile` feature, as `profiling::scope!` did, and Flint's own scope recorder,
which every debug build has and a release build gets with flint-player's new
`trace` feature (`trace = ["flint-player/trace"]` in a game's binary). Nothing
records yet: the recorder's file and flags arrive with #1097. A default
release build carries none of it. In Tracy the engine's zones are now named
by their function alone (`tick`, `render`, `update_from_world`), not by its
module path.
**What to do.** Nothing. To have a game's own scopes reach the recorder too,
write `flint_core::scope!` where it wrote `profiling::scope!` or
`#[profiling::function]`; the old forms still reach Tracy.
