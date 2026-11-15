# ADR 0055: Every projection and depth reader uses wgpu's `[0, 1]` depth

Status: accepted, 2026-08-20 (commit
[`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)).
Backfilled 2026-11-12 (#374) from commit `aac2d27` (message and diff) and
the code and book lines that cite it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Part of the Tier 2 renderer pass ([ADR 0054](0054-tier-2-renderer-pass-scope.md)),
listed as "wgpu depth-convention fix (tech-debt #12)". The perspective
matrix used OpenGL's `[-1, 1]` depth rows, while the orthographic matrix
already mapped to wgpu's `[0, 1]`. Under wgpu's clip rules the GL rows
clipped geometry nearer than twice `near`, and depth linearized with the GL
formula came out halved, so DoF focus values drifted with the far plane.

## Decision

- `Camera::perspective_matrix` maps view depth `-near` to 0 and `-far` to 1
  (`-far / (far - near)`, `-(far * near) / (far - near)`), matching
  `orthographic_matrix`.
- Every depth consumer moves with it: the frustum's near plane is row 2
  (it was row 3 + row 2), and the composite shader's `linear_depth`, the
  ocean's depth inversion, the shadow cascades' pairing and the viewer's
  DoF-follow all assume `[0, 1]`.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Depth-of-field distances are plain view-space metres:
  `dof_focus_distance = 10` is ten metres from the camera. The viewer's
  DoF-follow reports the same values.
- Geometry between `near` and `2 * near` is no longer clipped.
- Since then: the sky test is `depth >= 1.0` ([ADR 0078](0078-fog-reaches-everything-drawn.md)), which relies on
  this convention. The Capitol's fog work (2026-10-16) found the ocean's
  self-fog workaround still described GL-style depth.

## Sources

- Commit
  [`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)
  "Batch: F4 Rendering & Effects debug menu + Tier 2 renderer pass +
  prologue variants (ADRs [0053](0053-one-rendering-and-effects-menu-on-f4.md)-[0061](0061-flight-trails-from-script-driven-emitters.md))"
- Vault: "The Capitol - Fog Reaches Everything (H10) - 2026-10-16"
- Code now: `crates/flint-render/src/camera.rs` (`perspective_matrix`),
  `crates/flint-render/src/frustum.rs`,
  `crates/flint-render/src/composite_shader.wgsl` (`linear_depth`),
  `crates/flint-render/src/ocean_shader.wgsl`,
  `crates/flint-render/src/shadow.rs`
- Book: [Post-Processing](../book/src/concepts/post-processing.md), "Depth
  of Field"
