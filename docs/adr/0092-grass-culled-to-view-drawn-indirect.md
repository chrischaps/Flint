# ADR 0092: Grass is culled to the view and drawn indirect, with no count read-back

Status: accepted, 2026-10-18 ([#64](https://github.com/chrischaps/Flint/pull/64)).
Backfilled 2026-11-12 (#374) from PR #64, whose body holds the ADR text, and
commit `91664bb`. Follows [ADR 0089](0089-compute-grass-over-any-ground.md) (grass over any ground).

## Context

[ADR 0089](0089-compute-grass-over-any-ground.md) dispatches only the grid cells within `max_distance` of the camera,
but two costs were left (TheCapitol#133):

1. Every blade in reach was drawn, behind the camera too. Standing in an
   unbroken field, about three quarters of the disc is out of view.
2. The CPU waited for the GPU every frame. `render_to` submitted the compute
   pass on its own, then `read_grass_instance_count` mapped a staging buffer
   with `device.poll(Maintain::Wait)` to learn how many instances to draw.

## Decision

- **Frustum cull in `cs_main`.** The compute uniforms carry the camera's six
  planes, and a blade is kept only if its root is inside all of them. Each
  plane is first pushed out by `grass_cull_padding`: the tallest blade
  (`blade_height × (1 + height_variation)`), and with shadows on, the reach
  of its shadow along the ground, height ÷ tan(sun elevation), capped at
  eight blade heights. One cull serves the main pass and the shadow cascades,
  so there is still one instance list.
- **Indirect draw.** The atomic counter becomes the `instance_count` of a
  `DrawIndexedIndirect` buffer (`GrassDrawArgs`), reset each frame. The main
  pass and both grass shadow cascades call `draw_indexed_indirect`. When the
  instance buffer is full a blade gives its slot back (`atomicSub`), so the
  count never runs past the buffer.
- **The compute runs before the shadow pass.** It used to run after, so the
  cascades drew the previous frame's blades and the shadows swayed a frame
  behind.
- **Stats without waiting.** `GrassCountReadback` copies the count into a
  4-byte buffer and maps it with `map_async`, picking the value up on a later
  frame with `poll(Maintain::Poll)`. `RenderStats.grass_instances` lags by a
  frame or two.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Nothing on screen is lost. With `wind_strength` 0, a fixed-step frame from
  either build is pixel-identical, shadows included; with wind, the only
  differences are the old one-frame shadow lag.
- A sun lower than about 7° casts blade shadows longer than the padding, and
  those can be clipped at the frame's edges.
- `draw_indexed_indirect` needs `DownlevelFlags::INDIRECT_EXECUTION`, which
  ships with compute on every backend grass runs on. With no compute, grass
  is off, as before.
- `SceneRenderer::read_grass_instance_count` is gone.
- The never-wait read-back became the pattern for GPU timing
  ([ADR 0108](0108-gpu-timing-from-timestamp-queries.md)).
- Since then: the frame goes to the GPU in one submission (#146, for #139).
  The grass read-back starts from `SceneRenderer::frame_submitted` after the
  host submits the frame.

## Measured

RTX 4070, uncapped, builds from before and after run alternately. Grass cost
is frame time with grass minus frame time without it.

| Case | Before | After |
|---|---|---|
| Unbroken field (density 40, 70 m reach), 1600×900 | 507k blades, 3.8 ms | 125k blades, 1.1 ms |
| Mesh Meadow demo, 1600×900 | 111k blades, 1.3 ms | 83k blades, 0.9 ms |
| The Capitol's rural field, 1920×1080, frame at eye height | 3.6 ms | 3.3 ms |

## Sources

- [#64](https://github.com/chrischaps/Flint/pull/64) / commit
  [`91664bb`](https://github.com/chrischaps/Flint/commit/91664bbb587febae38d6c03fd25d8f4c09e7656c)
  "Grass culled to the view and drawn indirect (ADR 0092)"
- Code now: `crates/flint-render/src/grass_compute.wgsl`,
  `crates/flint-render/src/grass_pipeline.rs` (`grass_cull_padding`,
  `GrassDrawArgs`, `GrassCountReadback`),
  `crates/flint-render/src/scene_renderer/render_passes.rs`
  (`dispatch_grass_compute`)
- Book: [Grass on Any Ground](../book/src/concepts/grass-ground.md), "Only
  what the camera sees"
