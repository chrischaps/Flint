# ADR 0085: Script drawing over egui: `push_over_ui` / `pop_over_ui`

Status: accepted, 2026-10-17 ([#53](https://github.com/chrischaps/Flint/pull/53)).
Backfilled 2026-11-12 (#374) from the ADR text in PR #53's description and
commit `8a4dfd7`.

## Context

Flint paints the script HUD (`draw_*` from Rhai) on egui's background layer,
before any game panel, so a game's egui always covers it. Usually that's
right: a panel's contents shouldn't be hidden under HUD drawing. But some
short-lived effects have to reach the UI. In The Capitol, a gathered item's
grade pips fly to the inventory. The header strip and the inventory panel
are egui, so the pip stopped under the header's Inventory button, and slid
out of sight behind an open panel.

## Decision

Add `push_over_ui()` and `pop_over_ui()` to Rhai. Every draw call issued
between them is stamped `over_ui` on its `DrawItem`, the way `push_clip`
stamps a clip. `render_draw_commands` paints those items with a second
painter, on `LayerId::new(Order::Middle, "flint_script_over_ui")`.

- egui paints each `Order` back to front. Within one order it paints the
  areas (windows, popups) in stacking order first, then any layer that
  isn't an area. This layer is never an area, and it's the only non-area
  layer on `Middle`. So it always lands above every game panel
  (`Background`) and window (`Middle`), and below popups, debug panels
  (`Foreground`) and tooltips.
- Within the layer, items still sort by `layer`, and each can be clipped.
- Pushes nest, and an unbalanced pop is ignored. The depth resets with the
  clip stack every frame, so a script that forgets to pop can't leak into
  the next frame.

## Alternatives considered

- A `layer` threshold (say ≥ 1000 means over the UI) needs no new
  functions, but it's a magic number, and it mixes "draw order among
  scripts" with "which side of the UI".
- An `over_ui` key in each `*_opts` map would cover only the few functions
  that take options.
- A whole extra HUD pass after egui would cost another render pass and its
  own text pipeline, to get what one egui layer already gives.

## Consequences

- Opt-in and backward compatible: nothing changes for scripts that don't
  call it.
- Over-UI drawing gets no egui input. It's paint only, like the rest of the
  script HUD.
- Another non-area painter on `Order::Middle` would share that order with
  this layer, in hash-map order. None existed in Flint then.

## Sources

- [#53](https://github.com/chrischaps/Flint/pull/53) / commit
  [`8a4dfd7`](https://github.com/chrischaps/Flint/commit/8a4dfd71db2ecd7a2c4ec4ddeb5ac11dd94e2455)
  "Script drawing over egui: push_over_ui / pop_over_ui (ADR 0085)"
- Not this decision: commit `974c998` "[H19] Flint: a placed sound's
  falloff curve, ... (ADR 0085)" also says 0085; it merged as
  [ADR 0086](0086-placed-sound-falloff-curve.md).
- Vault: "The Capitol - sound falloff where it really is (H20, GH-94) -
  2026-10-17"
- Code now: `crates/flint-script/src/context.rs` (`push_over_ui`),
  `crates/flint-player/src/player_app/hud_render.rs` (`over_ui_layer`);
  book: [Scripting](../book/src/concepts/scripting.md), "Over the UI"
