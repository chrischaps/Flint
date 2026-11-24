### A player without `living` links no wooze field; five Rust paths moved (#1330)
**What changed.** `flint-render` no longer depends on `flint-wooze`, and
`flint-player` and `flint-viewer` link it only with their `living` feature
(default on), so a build without living props carries none of the field.
Moved, for Rust that names them (the games in Flint's repos have none):
`flint_render::shallow_wooze::{ShallowWooze, GrassWooze, warn_without_a_field,
warn_grass_without_a_field}` are `flint_wooze::shallow::*`, and
`GrassWooze::advance` / `warn_grass_without_a_field` take the grass's
`GrassConfig::wooze_comb()` (new) instead of the config; `BoilFrame`,
`ShallowWoozeDraw` and `WAVELENGTH_MIN` are defined in
`flint_core::field_frame` (still re-exported from `flint_wooze` and
`flint_render`); `flint_core::components::LIVING_PROP` is
`flint_living::components::LIVING_PROP`, and `MUSIC_SESSION` is
`flint_music::components::MUSIC_SESSION`. Scenes, scripts and pictures are
unchanged.
**What to do.** Nothing, unless a game's own Rust names one of those paths:
change the `use` line.
