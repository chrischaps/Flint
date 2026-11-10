# flint-animation: two-bone IK (moved out of the root CLAUDE.md by #755)

The skinned pipeline gotcha: `crates/flint-render/CLAUDE.md`. Sequences and the model previewer: `crates/flint-cli/CLAUDE.md`.

- **Two-bone IK** (ADR 0070) -- `ik_two_bone` component on the second bone of a transform chain names `target`, `pole` and `tip` entities; `IkPass` (flint-animation) solves after scripts/physics/animation each frame and before the renderer reads world matrices, writing only the two bones' `rotation_quat` from their captured rest. Lengths come from the model's rest offsets. `flint render` runs it once
