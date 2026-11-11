# flint-animation: two-bone IK (moved out of the root CLAUDE.md by #755)

The skinned pipeline gotcha: `crates/flint-render/CLAUDE.md`. Sequences and the model previewer: `crates/flint-cli/CLAUDE.md`.

- **Nodes above a joint are part of its pose** (#294) -- `ImportedJoint::parent_offset` is the matrix of the non-joint glTF nodes between a joint and its parent joint (or the scene root), `Skeleton::parent_offsets` applies it before the local pose, so `global_matrices` are model space and bind-pose bone matrices are identity. Mixamo's `Armature` (scale 0.01, +90 X) rides there; skipping it made every animated YBot 100x and lying down, which scenes had hidden with `[90, yaw, 0]` / `0.015`. Test: `mixamo_armature_node_is_part_of_the_pose`
- **Two-bone IK** (ADR 0070) -- `ik_two_bone` component on the second bone of a transform chain names `target`, `pole` and `tip` entities; `IkPass` (flint-animation) solves after scripts/physics/animation each frame and before the renderer reads world matrices, writing only the two bones' `rotation_quat` from their captured rest. Lengths come from the model's rest offsets. `flint render` runs it once
