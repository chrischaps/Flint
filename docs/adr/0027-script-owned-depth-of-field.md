# ADR 0027: Depth of field is a single-pass blur in the composite shader, owned by scripts

Status: accepted, 2026-08-17 (commit
[`27c5ff4`](https://github.com/chrischaps/Flint/commit/27c5ff449142539780c1cb1d471090d6c3777302)).
Backfilled 2026-11-12 (#374) from commit `27c5ff4` and its shader diff, Vault
note "2026-08-17 DoF as a Diegetic Gradient (Starchild)", and the code.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.
Amended by [ADR 0055](0055-wgpu-zero-to-one-depth-convention.md) (depth convention) and [ADR 0053](0053-one-rendering-and-effects-menu-on-f4.md) (the F4 menu).

## Context

Star Child shows no meters. Its tips document named depth of field as the
first-choice gradient for lean error: the world pulls into focus as the lean
lands. Flint had no DoF.

## Decision

- DoF is a single-pass, CoC-weighted disc gather in the composite shader:
  12 golden-angle taps, radius up to 10 px, each tap weighted by its own
  circle of confusion so an in-focus foreground doesn't bleed into a
  defocused background. It gathers from the raw HDR source, as radial blur
  does, so the two blurs stay order-stable.
- Three fields, `dof_strength`, `dof_focus_distance`, `dof_focus_range`, in
  `[post_process]`, the renderer's config, and `flint render --dof`,
  `--dof-focus`, `--dof-range`.
- Scripts set it with `set_dof(strength)` and `set_dof_focus(distance,
  range)`.
- DoF is script-owned: the music ladder's post merge never writes it. There
  is no restore path, because an owning script writes it every frame and a
  scene load resets the sticky config from the scene.
- The prologue drives it from the one lean-error parameter: the focus plane
  rides the camera-to-protagonist distance and the error widens the defocus.

The Vault note says the effect was cheap because the composite pass already
bound depth for fog and already had near and far in its uniforms.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The ladder owns "the world is draining" (desaturation, [ADR 0021](0021-ladder-visuals-through-the-post-stack.md)); the
  player's own accuracy owns clarity (DoF). The Vault note pairs them on
  those two axes.
- The commit left tuning to Chris.
- Since then: the shader linearized depth with the wgpu `[0, 1]` formula
  against a GL-convention projection, so focus values read about half the
  true distance (tech-debt #12, found in the clay-look spike, [ADR 0047](0047-viewer-dof-follow.md)).
  [ADR 0055](0055-wgpu-zero-to-one-depth-convention.md) moved every depth consumer to one convention, and
  `dof_focus_distance` is now plain view-space metres. [ADR 0053](0053-one-rendering-and-effects-menu-on-f4.md) put the DoF
  controls in the F4 Rendering & Effects menu.

## Sources

- Commit
  [`27c5ff4`](https://github.com/chrischaps/Flint/commit/27c5ff449142539780c1cb1d471090d6c3777302)
  "F6 + diegetic followups: prototype/prologue scenes, next-target lookahead,
  audio gradient, haptics, depth-of-field"
- Vault: "2026-08-17 DoF as a Diegetic Gradient (Starchild)"
- Vault: "2026-08-19 Viewer DoF Follow (F12)"
- Code now: `crates/flint-render/src/composite_shader.wgsl` (`coc_at`),
  `crates/flint-script/src/api/post_process.rs` (`set_dof`, `set_dof_focus`),
  `crates/flint-player/src/player_app/frame.rs` (`drain_post_overrides`),
  `crates/flint-cli/src/commands/render.rs` (`--dof`),
  [Post-Processing](../book/src/concepts/post-processing.md), "Depth of Field"
