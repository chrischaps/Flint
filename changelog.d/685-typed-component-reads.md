### Physics and audio say when a value is wrong, and an integer is a number (ADR 0115, #685)
**What changed.** Physics and audio now log a wrong component value once per
entity and field instead of taking the default in silence: an unknown
`rigidbody.body_type`, `collider.shape`, `joint.type` or `audio_source.bus`
(with the allowed values and the nearest one), a value of the wrong type, and
a `collider` on an entity with no `rigidbody`, which has never collided
(`needs-component`). What they fall back to is unchanged. Two behaviours do
change: `character_controller.move_speed`, `jump_force` and `height` written
as integers (`move_speed = 8`) used to read as the defaults (5.0, 6.0, eye
height 1.5) and are now read as written, and `material.base_color_r/g/b/a`
written as integers used to drop the tint and now apply it.

**What to do.** Run the game and read the log for `flint_ecs::view` lines;
each says what to write. If a character moves or jumps differently, or a
model gains a tint, the scene had an integer the engine ignored: write the
value you want. Rust in a game binary can read the same way with
`world.view(id, "collider").f32("friction", 0.5)`
([book](../docs/book/src/guides/player-extensions.md#reading-components)).
