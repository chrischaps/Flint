### `register_state` refuses what nothing runs; `living` has its own key (#727)
**What changed.** A state names systems (`physics`, `scripts`, `animation`,
`particles`, `audio`, `living`), each `"run"` or `"pause"`. Any other key, any
other policy, and `rendering`, `transparent` and `"hidden"` (documented, never
implemented: the frame always draws) are now a script error at the call, and
the state isn't registered. Living props pause on `living`, not `animation`;
`paused` and `loading` pause everything, the grove included. A script's
`fire_event` now survives a scene transition. In Rust: `StateConfig` is a map
(`runs("physics")`, `with(..)`), `register_state` returns a `Result`,
`RuntimeSystem` is `initialize` and `update` only, and
`PhysicsSystem::fixed_update` takes the caller's `&mut EventBus`
(`push_event`/`drain_events` are gone).
**What to do.** Delete `rendering:` and `transparent:` from `register_state`
maps. Add `living: "pause"` to a state that should still the grove.
