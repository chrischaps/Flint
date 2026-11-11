### Every script function is declared: styles, particles, splines, ocean, saves and the rest take integers and check their entity (#695)
**What changed.** The rest of the script API is declared like the Entity
API, and `flint script api` lists each function's module (`debug` holds the
Living Props A/B calls and `commit_living`). A number takes an int
(`play_effect("spark", 0, 1, 0)`, `ocean_height(x, 0)`) and an int a whole
float (`emit_burst(id, 3.0)`), where both were "Function not found". The
particle emitter calls, the Style API's entity functions and `spline_*`
given `-1` or a despawned id do nothing, queue nothing and warn once.
`tween_style_param` with non-number seconds is a script error. For Rust: the
`EmitBurst`, `SetEntityStyle`, `SetStyleParam`, `TweenStyleParam` and
`ClearEntityStyle` commands carry `entity: EntityId` for `entity_id: i64`,
and `ScriptCallContext`'s `unsafe` `world_ref`/`world_mut`/`physics_ref` are
gone (`try_world`, `try_world_mut`, `try_physics`).
**What to do.** Nothing for a script that worked; `.to_float()` added only
for these functions can go. Check a maybe-missing entity with
`entity_exists(id)`. Rust that matches those commands renames `entity_id`
to `entity`.
