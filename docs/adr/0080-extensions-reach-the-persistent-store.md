# ADR 0080: Player extensions reach the persistent store

Status: accepted, 2026-10-17 ([#48](https://github.com/chrischaps/Flint/pull/48)).
Backfilled 2026-11-12 (#374) from the ADR text in PR #48's description and
commit `fcf1ebb`.

## Context

`PlayerApp` owns the script-facing `PersistentStore`. It loads it before
any `on_init`, saves it 1 s after a burst of changes, on scene transitions
and on exit, and hands scripts a pointer to it during their calls.
`ExtensionCtx` ([ADR 0071](0071-player-extensions-in-a-game-binary.md))
exposed the world, clock, input, camera and GPU, but not the store. An
extension wanting settings that scripts also read had to keep its own file
and mirror values into scripts through native functions.

## Decision

`ExtensionCtx` has `pub persist: &'a mut PersistentStore`, available in
`pre_update`. No new API on the store: extensions use `get`/`set`/`remove`
as the player does, and `request_flush()` to save at the end of the frame,
as a script's `persist_save()` does. Saving stays the player's job. Its
per-frame flush already checks `is_dirty()`, so extension writes join the
debounce, the transition save and the exit save with no extra wiring.

## Alternatives considered

- Widening `ui` and `on_window_event` to reach the store too. Rejected: it
  would break every extension for a one-frame difference.

## Consequences

- One store, one file, one save policy for a game's Rust and its scripts.
- Ordering is the frame's: an extension's write is visible to that frame's
  `on_update`, and a script's write to the next frame's `pre_update`.
- The store is only reachable from `pre_update`. A setting changed in egui
  is written on the next `pre_update`.
- Every script can read every key, so the docs say to keep secrets
  (session tokens) in a file of the game's own.
- `ExtensionCtx` can't be built outside `flint-player`, so the new field
  broke no one.

## Sources

- [#48](https://github.com/chrischaps/Flint/pull/48) / commit
  [`fcf1ebb`](https://github.com/chrischaps/Flint/commit/fcf1ebb58a07519b735081f9b4733a5f315aea56)
  "Extensions reach the player's PersistentStore (ADR 0080)"
- Vault: "The Capitol - Extensions Reach the Persist Store (H13) -
  2026-10-17"
- Code now: `crates/flint-player/src/extension.rs` (`ExtensionCtx::persist`);
  book: [Player Extensions](../book/src/guides/player-extensions.md), "Saved
  data"
