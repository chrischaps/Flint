# ADR 0095: A game picks its ground with a ray against a placed height field

Status: accepted, 2026-10-18 ([#67](https://github.com/chrischaps/Flint/pull/67)).
Backfilled 2026-11-12 (#374) from PR #67, whose body holds the ADR text,
commit `9821ad9`, and the Vault note "The Capitol - picking the ground on a
hill (GH-201, W8)".

## Context

Picking used a horizontal plane, `screen_to_ground(x, y, plane_y)`. On a hill
a click lands past or short of the spot under the cursor, and a camera rig
can't tell how high the land is under it (TheCapitol#201, W8). Games with
their own ground (built from a function, a server or a mesh, not a scene
`terrain`) had no way to pick or sample it from scripts. The only height
query was `terrain_height`, tied to the scene's terrain. The editor had a ray
against a heightmap, but it assumed the terrain sat at the origin and missed
rays that started off it.

## Decision

- A placed height field, `flint_terrain::GroundField`: a `Heightmap` whose
  first sample is at `origin` and last at `origin + size`, with world y
  `sample × height_scale + base_y`. It has `height_at(x, z)` and
  `ray(origin, dir)`.
- One march serves the game, scripts and the editor: the ray is clipped to
  the field's box, stepped a texel at a time over the ground, and the
  crossing step bisected (24 rounds). A ray can start anywhere.
  `ray_heightmap_intersect`, the editor's brush ray, is a thin wrapper over
  it.
- The game registers the field with the `Projector`, the handle that already
  owns the scripts' camera ([ADR 0087](0087-native-code-projects-with-frame-camera.md)), so picking uses the same camera as
  `world_to_screen`: `Projector::set_ground`, `screen_to_heightfield`,
  `ground_height`; `View::screen_ray`. The field is swapped whole (an `Arc`),
  survives `clear()`, and the march runs without the lock.
- Scripts: `screen_to_heightfield(x, y)` returns `[x, y, z]` or `()`, and
  `ground_height(x, z)` a number or `()`. Misses return `None`/`()`, and the
  caller chooses its own fallback (usually `screen_to_ground`).
- `flint-player` re-exports `GroundField` and `Heightmap`.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The pick is exact to bisection on the bilinear surface; a test checks a
  click on a slope lands within one texel.
- The cost is one step per texel crossed inside the box. A field the size of
  The Capitol's (12 km at a few metres a texel) is a few thousand steps for a
  grazing ray, far fewer looking down.
- Features thinner than a texel can be stepped over.
- Chunked or streamed fields aren't covered: a game swaps in a new field
  when it rebuilds.
- `flint-script` depends on `flint-terrain` (no editor code).
- A ray that starts off the terrain no longer misses it in the editor.

## Sources

- [#67](https://github.com/chrischaps/Flint/pull/67) / commit
  [`9821ad9`](https://github.com/chrischaps/Flint/commit/9821ad98b820445844e23eb75ecf9dc7cffc8bec)
  "Pick a ground that isn't flat: a ray against a placed height field (ADR 0095)"
- Vault: "The Capitol - picking the ground on a hill (GH-201, W8) - 2026-10-18"
- Code now: `crates/flint-terrain/src/ground.rs` (`GroundField`),
  `crates/flint-terrain/src/brush.rs` (`ray_heightmap_intersect`),
  `crates/flint-script/src/projector.rs`, `crates/flint-script/src/api/view.rs`
- Book: [Player Extensions](../book/src/guides/player-extensions.md), "Picking
  a ground that isn't flat"; [Scripting](../book/src/concepts/scripting.md);
  [Terrain](../book/src/concepts/terrain.md)
