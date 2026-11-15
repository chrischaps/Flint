# ADR 0069: Physics joints, and rigidbodies placed in world space

Status: accepted, 2026-09-04 (commit
[`44e51af`](https://github.com/chrischaps/Flint/commit/44e51af0d15e2e1222a683f971e336293ccce930)).
Backfilled 2026-11-12 (#374) from commits `44e51af` and `1eb2bb6`, the Vault
note "2026-09-04 Flint Physics Joints And World Space Bodies", and the code
and book lines that cite it. This record was never a file; it lived in the
commit message and code comments.

## Context

The engine half of the Trike game's physics-joints plan
(`Trike/docs/physics-joints-plan.md`). The trike's tricks wanted scripted
primary motion with jointed secondary motion: bounce, recoil, wobble.
Flint had no joints, and a rigidbody was placed from its local transform, so
a body on a parented or glTF-expanded child node simulated somewhere other
than where it rendered. Dynamic boxes translated but never visibly tumbled.

## Decision

- **World-space bodies.** Every body is placed from `get_world_matrix`,
  rotation quaternion included. A dynamic body writes back a local
  `position` and `rotation_quat` against its parent's world matrix. Scale is
  not part of the physics chain: bodies, joints and their ancestors are
  assumed to have unit scale.
- **A `joint` component**: hinge, prismatic, spherical or fixed, with
  anchor, axis, limits and a position motor, synced to Rapier impulse
  joints. The jointed entity is the child (Rapier's `body2`); the parent is
  `joint.parent` by name or the transform parent. A joint is created once,
  when both bodies exist, and its motor target, gains and limits are
  retargeted in place. TOML is in degrees and metres; radians happen at the
  sync.
- **Rhai**: `rotate_local`, `set_rotation_quat`, `get_rotation_quat`,
  `set_joint_target`, `get_joint_target`, `get_joint_position`.
- **Colliders** gain a `cylinder` shape and `offset` and `rotation` fields.
- Shared quaternion and rigid-inverse helpers in `flint_core::quat`, which
  `flint-animation` now delegates to.

## Alternatives considered

Chris set the scope, from the Vault note: the core of the plan plus the
cheap colliders. `convex_hull`, glTF `extras` and blending scripts over
clips were deferred until the trike needs them.

## Consequences

- Bugs fixed on the way: physics composed Euler angles as `qx * qy * qz`
  while the renderer uses `Rz * Ry * Rx`, so a kinematic body with two
  non-zero angles was turned differently from its mesh; kinematic bodies
  applied the `bounds` centre twice; the schema parser didn't accept the
  `float` and `int` type names engine schemas use.
- A hinge whose parent and child rest rotations differ needs `local_axis1`
  in the parent's frame, or it binds (Vault gotcha).
- Script `set_field` didn't add a component to the world's index, so a
  joint or body created in `on_init` never got built. Commit `1eb2bb6`
  routed it through `World::set_field` the same day.
- `set_rotation` still deletes the quaternion; rest-pose nodes use
  `rotate_local` or `set_rotation_quat`.
- Since then: `rigidbody`, `collider` and `joint` are read through the
  typed component view
  ([ADR 0115](0115-scene-diagnostics-and-complete-schemas.md)), and
  `joint.type` also takes `slider` and `ball`.

## Sources

- Commit
  [`44e51af`](https://github.com/chrischaps/Flint/commit/44e51af0d15e2e1222a683f971e336293ccce930)
  "physics: joints, world-space bodies, rest-space rotation API (ADR 0069)"
- Commit
  [`1eb2bb6`](https://github.com/chrischaps/Flint/commit/1eb2bb698d11031cfb0b349b7cd13a9815dc0a32)
  "engine: script set_field maintains the component index"
- Vault: "2026-09-04 Flint Physics Joints And World Space Bodies"
- Code now: `crates/flint-physics/src/sync.rs`,
  `crates/flint-physics/src/joint_tests.rs`, `schemas/components/joint.toml`,
  `examples/joint_test.scene.toml`
- Book: [Physics and Runtime](../book/src/concepts/physics-and-runtime.md),
  "Joints"
