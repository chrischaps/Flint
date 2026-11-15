# ADR 0091: Shoreline foam is measured across the water, by a screen-space search

Status: accepted, 2026-10-18 ([#63](https://github.com/chrischaps/Flint/pull/63)).
Backfilled 2026-11-12 (#374) from PR #63, whose body holds the ADR text,
commit `22cac3a`, and the Vault note "The Capitol - shoreline foam across the
water (GH-132)". Follows [ADR 0088](0088-shoreline-foam-from-scene-depth.md) (shoreline foam).

## Context

[ADR 0088](0088-shoreline-foam-from-scene-depth.md) foams water where the opaque point behind it, on the same view ray,
lies within `shore_foam_width` of the surface. That is a depth along the ray,
so on a vertical wall the band covers `width / tan(elevation)` of water:

- From a low camera the band is broad: on the shoreline demo, about a metre
  in front of each pier where 0.35 m was asked for.
- From overhead it's a hairline, and on The Capitol's canals it vanished
  from both walls (TheCapitol#132).
- At the foot of the wall nearer the camera, the ray looks away from the
  wall to the bed, so that wall never foams.

## Decision

A screen-space shore search, behind a new `ocean` field, `shore_foam_reach`
(metres; 0 = off, the default, so existing scenes are unchanged).

With a reach, each water pixel inside the foam's fade distance (140 m):

- Reads the grab pass depth at 24 pixels round it: 8 directions, 3 steps out
  to the band's widest (`1.25 × width`) in pixels at its depth, capped at
  32 px. A band under a pixel wide skips the search.
- Rebuilds each tap's scene point. The pixel's ray divided by its view depth
  is linear in screen space, so `dpdx`/`dpdy` of it (taken in uniform control
  flow) step it to any tap, with no inverse matrix in the uniforms.
- Measures to the nearest point that is shore: `max(distance across the
  water, depth below the surface)`. The pixel's own ray point counts too. A
  wall is then measured across the water and a gentle beach by its depth.
- Solids more than `reach` above the surface aren't shore: a bridge deck, a
  person on the quay, a hull's superstructure. Without this cap, any overhang
  paints foam on the water under its edge in plan.

The field takes the uniform's first spare pad (`_pad_shore0`), so the struct
stays 848 bytes.

## Alternatives considered

- **A game-supplied shore-distance mask** over a world rectangle. Exact, and
  it covers off-screen walls. But the game has to rasterise it, and a thin
  band needs fine texels: the Capitol's 14 cm band over an 800 m world would
  want about 2 cm texels.

## Consequences

- A wall's band is `width` wide from any angle, and both walls foam from
  overhead. The near wall's band is found from its kerb.
- It only finds shores on screen. A wall just outside the frame's edge
  doesn't foam the water beside it.
- At a vertical wall the band no longer rises and falls with the wave height;
  the surge and the noise still lap it. Beaches behave as before.
- `examples/shoreline.scene.toml` (then `demo/`) sets `shore_foam_reach =
  1.8`: the quay is 1 m up, the piers 1.6 m.
- Since then: [ADR 0099](0099-water-body-scales-shore-band.md) scales the
  band, and the search's radius with it, per water body.

## Measured

- Cost: up to 24 `textureLoad`s of R32F per water pixel within 140 m. On an
  RTX 4070 at 1920×1080, with a canal filling much of the frame, the A/B was
  within noise: about 2.9 ms a frame either way, uncapped.
- `flint render` of the shoreline demo from 80° down: with reach 0 the middle
  pier has no foam and the others hairlines; with 1.8 each has a full ring.

## Sources

- [#63](https://github.com/chrischaps/Flint/pull/63) / commit
  [`22cac3a`](https://github.com/chrischaps/Flint/commit/22cac3add4c2dec589859dd4296fb95ff7291a1f)
  "Shoreline foam: measure the band across the water (ADR 0091)"
- Vault: "The Capitol - shoreline foam across the water (GH-132) - 2026-10-18"
- Code now: `crates/flint-render/src/ocean_shader.wgsl` (the search),
  `crates/flint-render/src/ocean_pipeline.rs` (`shore_foam_reach`),
  `schemas/components/ocean.toml`, `examples/shoreline.scene.toml`
- Book: [Ocean](../book/src/concepts/ocean.md), "Shoreline foam"
