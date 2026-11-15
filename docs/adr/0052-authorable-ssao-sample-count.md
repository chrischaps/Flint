# ADR 0052: SSAO's sample count is a scene-authorable lever

Status: accepted, 2026-08-19 (commit
[`7d0144b`](https://github.com/chrischaps/Flint/commit/7d0144bfc1d7472ea8f21b688218dafac595a8cf)).
Backfilled 2026-11-12 (#374) from commit `7d0144b` and the book lines that
cite it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

SSAO profiled as the renderer's largest per-pixel cost: 64 hemisphere
samples a pixel at full resolution, plus the blur. The count was fixed.

## Decision

- `[post_process] ssao_samples` (1-64, default 64) and `flint render
  --ssao-samples`. It rides `SsaoUniforms`' former pad slot.
- The shader strides through the scale-ordered kernel
  (`kernel[(i * 64) / sample_count]`), so a reduced count still covers the
  full radius; 64 is the identity. Out of range falls back to the full
  kernel.
- Chris's [H] verdict: the three clay look scenes author 16, about four
  times cheaper. The smoke gate scene stays at 64.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- Scenes that don't set it render as before.
- Since then: `ssao_samples` is also a quality cap
  (`QualitySettings::ssao_samples`, clamped 1-64, FlintKart#55), and the F4
  menu edits it
  ([ADR 0053](0053-one-rendering-and-effects-menu-on-f4.md)).

## Measured

From the commit's gates (same binary, `png_diff`):

| Comparison | Result |
|---|---|
| default vs explicit 64 | SHA-identical |
| 1 sample vs 64 | max delta 110 (the uniform is live) |
| 64 vs 16, clay solid hero | max delta 15/255 |

## Sources

- Commit
  [`7d0144b`](https://github.com/chrischaps/Flint/commit/7d0144bfc1d7472ea8f21b688218dafac595a8cf)
  "SSAO sample-count lever (ADR 0052): scene-authorable ssao_samples, clay
  scenes adopt 16"
- Code now: `crates/flint-render/src/ssao_shader.wgsl`,
  `crates/flint-render/src/postprocess.rs` (`ssao_samples`),
  `crates/flint-core/src/quality.rs`
- Book: [Post-Processing](../book/src/concepts/post-processing.md), "SSAO";
  [Headless Rendering](../book/src/guides/headless-rendering.md)
