# ADR 0072: Custom events reach scripts through `on_event(name, data)`

Status: accepted, 2026-10-11 ([#41](https://github.com/chrischaps/Flint/pull/41)).
Backfilled 2026-11-12 (#374) from commit `e311e06`, PR #41's description and
the Vault note "2026-10-11 Capitol Flint Scripts Hear Custom Events".
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

`GameEvent::Custom` reached audio but never reached scripts. The Capitol's
network layer needed a pipe from Rust to Rhai, to tell scripts "your
inventory changed" or "a trade request arrived".

## Decision

- A new script callback, `on_event(name, data)`.
  `ScriptEngine::process_events` broadcasts each `GameEvent::Custom` to
  every script that defines it, in entity-id order. The sender hears its
  own event. Arity is checked with the other callbacks.
- `data` is always a Rhai map. A non-table payload is wrapped as
  `#{ value: .. }`.
- `ExtensionCtx::fire_event(name, impl Into<toml::Value>)`. Fired events go
  onto the physics event queue after `pre_update`, so scripts see them in
  the same frame, before `on_update`. Script-fired events still arrive the
  next frame, because script commands drain after `on_update`.

## Alternatives considered

- HashMap order for delivery. Entity-id order was chosen so delivery is
  deterministic (`on_action` was left in HashMap order).
- A Rhai `Map` argument to `fire_event`. It takes `impl Into<toml::Value>`
  to match `GameEvent::Custom`'s payload type.

## Consequences

- The payload is `toml::Value`, which has no null. Server JSON forwarded to
  scripts has to turn `null` into an absent key.
- A listener that fails doesn't block the rest (`script_events.rs`).
- Still holds: `call_event_on_all` in `flint-script` wraps non-table data
  as `#{ value: .. }`.

## Sources

- [#41](https://github.com/chrischaps/Flint/pull/41) / commit
  [`e311e06`](https://github.com/chrischaps/Flint/commit/e311e0697a241af05889d2d9452753d09c4e6782)
  "[P1.2] H2: deliver custom events to scripts via on_event(name, data)
  (Flint)"
- Vault: "2026-10-11 Capitol Flint Scripts Hear Custom Events"
- Code now: `crates/flint-script/src/engine/callbacks.rs`
  (`process_events`, `call_event_on_all`), `crates/flint-player/src/extension.rs`
  (`ExtensionCtx::fire_event`), `crates/flint-script/tests/script_events.rs`;
  book: [Player Extensions](../book/src/guides/player-extensions.md),
  "Talking to scripts"
