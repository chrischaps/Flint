# ADR 0107: A game adds its own debug panels through its player extension

Status: accepted, 2026-10-21 ([#83](https://github.com/chrischaps/Flint/pull/83)).
Backfilled 2026-11-12 (#374) from PR #83, whose body holds the ADR text for
0107 to 0109, commit `35afb2a`, and the Vault note "2026-10-21 Capitol Flint
Learns To Time Its Frame".

## Context

A game that wraps the player draws its UI with `PlayerExtension::ui`, but its
dev views (net stats, an economy, a town's state) had nowhere to go in the F3
family. A `DebugPanel` the game built itself would have to share the game's
state through an `Arc<Mutex<_>>`, because the player owns the panel and the
extension separately (TheCapitol#374, the Observatory plan's D5.4).

## Decision

Two defaulted methods on `PlayerExtension`:

- `debug_panels(&self) -> Vec<String>` names the game's panels. The player
  asks when it builds its panels: each scene load, and `add_extension`.
- `debug_panel_ui(&mut self, name, ui)` draws one panel's contents, every
  frame it's open, after the extension's `ui`.

For each name the player holds a small `ExtensionPanel` (a name, an index, an
open flag) after the engine's panels. `show_debug_panels` takes a draw
callback, which sends an `ExtensionPanel` to its extension and lets every
other panel draw itself. F3 toggles the game's panels with the rest, they
dock in the same columns, opening one frees the cursor, and one open across a
rebuild stays open.

## Alternatives considered

- **A `DebugPanel` the game builds itself.** It would need the game's state
  behind an `Arc<Mutex<_>>`.

## Consequences

- Opt-in: an extension that names nothing changes nothing.
- The panel reads the extension's own state directly.
- With `debug-hud` off, neither method is called, and the trait keeps its
  shape.
- Names are read at rebuilds, so a name added later shows at the next scene
  load.
- Shipped in one PR with [ADR 0108](0108-gpu-timing-from-timestamp-queries.md)
  and [ADR 0109](0109-tracy-frame-profiler.md), which share the F2/F3
  surface; each touches its own code and can be reverted alone.

## Sources

- [#83](https://github.com/chrischaps/Flint/pull/83) / commit
  [`35afb2a`](https://github.com/chrischaps/Flint/commit/35afb2a254c411881cf581bc4e2f5be134cbbef6)
  "[#374] Flint: game debug panels, GPU timing, a frame profiler (ADRs 0107-0109)"
  (the `#374` is TheCapitol's issue, not Flint's)
- Vault: "2026-10-21 Capitol Flint Learns To Time Its Frame"
- Code now: `crates/flint-player/src/extension.rs` (`debug_panels`,
  `debug_panel_ui`), `crates/flint-player/src/player_app/debug_panels.rs`,
  `crates/flint-player/src/player_app/scene_entry.rs`
- Book: [Debug Panels](../book/src/guides/debug-panels.md), "Adding a panel
  from a game"; [Player Extensions](../book/src/guides/player-extensions.md)
