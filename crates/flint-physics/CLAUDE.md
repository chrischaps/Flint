# flint-physics: joints (moved out of the root CLAUDE.md by #755)

The Rapier version gotcha and the fixed timestep are in the root file. A character's starting facing: `crates/flint-player/CLAUDE.md`.

- **Physics joints** (ADR 0069) -- `joint` component (hinge/prismatic/spherical/fixed) on a dynamic entity, attached to `joint.parent` or the transform parent. Bodies are placed from `get_world_matrix` and dynamic poses write back as local `position` + `rotation_quat`; unit scale is assumed on the physics chain. TOML units are degrees/metres, converted to Rapier radians at sync. Scripts drive `motor_target` via `set_joint_target` and pose rest-quaternion nodes with `rotate_local` / `set_rotation_quat` (`set_rotation` still deletes the quaternion). Bench: `demo/joint_test.scene.toml`
