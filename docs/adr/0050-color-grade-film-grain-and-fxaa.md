# ADR 0050: Color grade, film grain and FXAA as scene-authored post levers

Status: accepted, 2026-08-19 (commit
[`95a260a`](https://github.com/chrischaps/Flint/commit/95a260a27da8ac5a2ceaed059c6af1ce2de8fb76)).
Backfilled 2026-11-12 (#374) from commit `95a260a` and the code and book
lines that cite it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

The "post trio" of the clay-look Tier 1 levers in commit `95a260a` (ADRs
0048-0050). Like the other five, each had to change nothing until a scene
asked for it, because the headless pixel-diff gates compare renders byte for
byte.

## Decision

- **Color grade.** `[post_process] grade_lift`, `grade_gamma`,
  `grade_gain`: ASC-CDL-shaped, `pow(max(color * gain + lift, 0), 1/gamma)`,
  applied right after ACES tonemapping. The grade is skipped by a
  `grade_enabled` flag the CPU computes when all three are neutral, never by
  trusting `pow(x, 1.0)` to be an identity.
- **Film grain.** `film_grain`: hash noise on pixel coordinates,
  luma-weighted so highlights stay clean, time-quantized to 24 Hz. It reads
  the shared post time (`mode_time`), which `flint render` fixes at 0, so
  headless renders stay deterministic (`--grain-time` picks another).
- **FXAA.** `fxaa`: FXAA 3.11 as its own pass after composite. Composite
  renders into an intermediate texture and FXAA writes the target. The
  pipeline and intermediate are created only when it is on (the lazy
  pattern Kuwahara used). Off by default, under tech-debt #13's interim
  clause: the cheap stand-in for the missing MSAA.
- **No Rhai setters**, by design. The surviving text doesn't say why.
- `PostProcessUniforms` grew from 224 to 272 bytes; `grain_intensity` took
  a former pad slot. Both layout tests were updated.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The same commit upgraded `shader_wgsl_parses` to a naga validate and
  brought `tools/png_diff.py` (in Starchild) for the gates, with
  `--ignore-rect` masks for the tavern's fireplace and dust-streak
  particles, which differ per run and per build. It reported same-binary
  renders identical with the levers off.
- Scripts still cannot set the grade, grain or FXAA; `post_process` declares
  vignette, bloom, exposure, chromatic aberration and radial blur.
- Since then: MSAA arrived the next day
  ([ADR 0058](0058-opt-in-4x-msaa-for-scene-passes.md)), and FXAA stays as
  an option beside it. #297 (PR #813) found FXAA was built only by the F4
  checkbox and on resize, so a hot reload that turned it on drew nothing;
  `SceneRenderer::encode_to` now calls `ensure_fxaa_resources` every frame.

## Sources

- Commit
  [`95a260a`](https://github.com/chrischaps/Flint/commit/95a260a27da8ac5a2ceaed059c6af1ce2de8fb76)
  "Clay-look Tier 1: all six gated levers (ADRs 0048-0050)"
- Vault: "2026-11-11 Flint FXAA Builds Itself Each Frame GH-297"
- Code now: `crates/flint-render/src/postprocess.rs` (`grade_lift`,
  `film_grain`, `fxaa_enabled`), `crates/flint-render/src/composite_shader.wgsl`,
  `crates/flint-render/src/fxaa_shader.wgsl`,
  `crates/flint-script/src/api/post_process.rs`
- Book: [Post-Processing](../book/src/concepts/post-processing.md), "Color
  Grade, Film Grain and FXAA"
