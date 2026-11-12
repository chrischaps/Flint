# ADR 0047: The viewer's DoF follow focuses on the selected entity, in the shader's depth units

Status: accepted, 2026-08-19 (commit
[`e3a4ff0`](https://github.com/chrischaps/Flint/commit/e3a4ff06816434c948ba038a0f2e77255e06fc27)).
Backfilled 2026-11-12 (#374) from commit `e3a4ff0` and its diff, Vault note
"2026-08-19 Viewer DoF Follow (F12)", and the code.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.
Amended by [ADR 0055](0055-wgpu-zero-to-one-depth-convention.md) (depth convention) and [ADR 0053](0053-one-rendering-and-effects-menu-on-f4.md) (the F4 menu).

## Context

Finding a DoF focus distance ([ADR 0027](0027-script-owned-depth-of-field.md)) by trial was slow. Under it sat a
known bug: the projection was GL-convention but the composite shader
linearized depth with the wgpu `[0, 1]` formula (tech-debt #12, found in the
clay-look spike), so a focus value read about half the true distance.

## Decision

- F12 in `flint edit` turns on DoF follow: the focus plane rides the last
  selected entity, kept after a deselect, with strength and depth-range
  sliders in a floating window. Only the three `dof_*` fields are touched,
  and writes are diff-gated. Turning it off restores the base config
  (authored post if on, else the viewer default).
- The viewer speaks the bug's units exactly instead of fixing it here. A
  surface at true view depth `z` reads back as `far·z / (2·far − z)`, so
  the focus is authored in that shader space, and the window shows both
  numbers. Values read off the window paste straight into `[post_process]`
  and `set_dof_focus`. One commented helper, to be deleted when the
  convention is fixed.

The Vault note gives the reason: a tool on top of a known bug that speaks
the bug's units exactly becomes a calibration aid for the fix.

## Alternatives considered

- **A rough ×0.5 conversion.** Rejected for the exact inverse, derived by
  composing the GL projection with the wgpu linearization (Vault).
- **Fixing the depth convention in the same change.** Not taken; it was
  left to tech-debt #12.

## Consequences

- The same commit made W, E and R gizmo shortcuts only while an entity is
  selected. Before, E (zoom in) was swallowed on its first press and only
  zoomed once key repeat started.
- Since then: [ADR 0055](0055-wgpu-zero-to-one-depth-convention.md) fixed the depth convention; `dof_focus_readout` now
  returns the plain view depth and the conversion is gone. [ADR 0053](0053-one-rendering-and-effects-menu-on-f4.md) moved
  the switch from F12 into the F4 Rendering & Effects menu as "DoF follow".

## Measured

- The exact inverse reproduced the spike's empirical pin: true depth 5.59
  gives focus 2.80, against an observed ≈2.85 (Vault).

## Sources

- Commit
  [`e3a4ff0`](https://github.com/chrischaps/Flint/commit/e3a4ff06816434c948ba038a0f2e77255e06fc27)
  "Clay-look spike + viewer post-process follow-ups (ADRs 0042-0047)"
- Vault: "2026-08-19 Viewer DoF Follow (F12)"
- Code now: `crates/flint-viewer/src/app.rs` (`dof_focus_readout`,
  `apply_dof_follow`, `set_dof_follow`),
  [Post-Processing](../book/src/concepts/post-processing.md), "Depth of Field",
  [Viewing](../book/src/getting-started/viewing.md)
