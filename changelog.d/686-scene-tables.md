### Scene-level tables have schemas, and a misspelled one warns (#686)
**What changed.** A scene's top-level tables (`[camera]`, `[environment]`,
`[post_process]`, `[living]`, `[wooze]`) are declared as `[table.<name>]` in
`schemas/scene/*.toml` and checked like components, by every scene load and
`flint validate`. A table no schema declares warns as `unknown-table`
(`[enviroment]`: did you mean `environment`?), a key a table doesn't declare
as `unknown-field`, and a bad value is an error: `[camera] projection =
"ortho"` used to draw in perspective without a word. `--strict` makes both
warnings errors. A game's own table (`[weather]`) is now kept by the loader:
declare it in `schemas/scene/weather.toml` and read it from a
`PlayerExtension` with `ctx.scene_table("weather")`. `wooze` and `wooze_zone`
are no longer components (`flint schema wooze` shows the table and its zones).
**What to do.** Run `flint validate` and read the new warnings; each names a
key that was doing nothing. Declare your own top-level tables. A game schema
that overrode `[component.wooze_zone]` becomes `[table.wooze]`'s `zone` field.
[Schemas](https://docs.chaps.dev/concepts/schemas.html#scene-level-tables) has the rest.
