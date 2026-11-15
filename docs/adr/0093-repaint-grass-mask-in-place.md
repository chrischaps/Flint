# ADR 0093: Part of a grass ground's mask can be repainted in place

Status: accepted, 2026-10-18 ([#65](https://github.com/chrischaps/Flint/pull/65)).
Backfilled 2026-11-12 (#374) from PR #65, whose body holds the ADR text,
commit `08218d5`, and the Vault note "The Capitol - grass mask patched in
place (H25, GH-135)". Follows [ADR 0089](0089-compute-grass-over-any-ground.md) and
[ADR 0092](0092-grass-culled-to-view-drawn-indirect.md).

## Context

`set_grass_ground` was the only way to change a ground's mask. Each call
uploads a new mask and height texture and rebuilds the instance buffer, the
draw-args buffer, the read-back and every grass bind group (`install_grass`).

The Capitol clears a small disc round each resource node in a 2048² mask.
Nodes arrive and leave as the player walks, so every change re-uploaded the
whole 4 MB mask and reinstalled the grass. The game throttled that to once a
second, so a node could wait a second for its clearing (TheCapitol#135).

## Decision

- `SceneRenderer::update_grass_mask(queue, x, y, width, height, bytes)`, and
  the same on `ExtensionGpu` without the queue. It writes the `width ×
  height` texels from texel (`x`, `y`) with `queue.write_texture`, one byte a
  texel, row by row toward +Z. Buffers and bind groups are untouched; the
  compute pass samples the mask every frame, so the change shows next frame.
- `set_grass_ground` keeps the mask texture, with its size. `install_grass`
  drops it, so a terrain's grass has none, and so do `unload_grass` and a
  scene transition.
- `Err` when no ground mask is installed, when the rectangle runs off the
  mask (overflow-safe), or when `bytes` is the wrong length
  (`check_grass_mask_patch`, public and pure). On a device with no compute
  grass it returns `Ok` and does nothing, as `set_grass_ground` does.
- Opt-in: nothing changes for a game that doesn't call it.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- A clearing in The Capitol became a write of about 17×17 texels (about 300
  bytes) instead of a 4 MB upload and a reinstall, and the game's throttle
  went away.
- `queue.write_texture` needs no row alignment, so any rectangle works.
- The mask texture lives as long as the grass, as it already did through its
  bind group. No new memory.
- Heights can't be patched this way. Nothing had needed that.

## Sources

- [#65](https://github.com/chrischaps/Flint/pull/65) / commit
  [`08218d5`](https://github.com/chrischaps/Flint/commit/08218d5a7d997182bd2f51dcd144d7bf11a1924a)
  "[#135] Flint: repaint part of a grass ground's mask in place (ADR 0093)"
- Vault: "The Capitol - grass mask patched in place (H25, GH-135) - 2026-10-18"
- Code now: `crates/flint-render/src/scene_renderer/mod.rs`
  (`update_grass_mask`), `crates/flint-render/src/grass_pipeline.rs`
  (`check_grass_mask_patch`), `crates/flint-player/src/extension.rs`
  (`ExtensionGpu::update_grass_mask`), `crates/flint-render/tests/runtime_mesh.rs`
  (`part_of_a_grass_mask_repaints_in_place`)
- Book: [Grass on Any Ground](../book/src/concepts/grass-ground.md)
