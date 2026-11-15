# ADR 0058: Opt-in 4x MSAA for the scene passes, with a sample-0 depth resolve

Status: accepted, 2026-08-20 (commit
[`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)).
Backfilled 2026-11-12 (#374) from commit `aac2d27` and the code and book
lines that cite it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Part of the Tier 2 renderer pass ([ADR 0054](0054-tier-2-renderer-pass-scope.md)),
listed as "MSAA CLI gate with sample-0 depth resolve". The renderer had no
MSAA; FXAA had been added the day before as the cheap interim (tech-debt
#13, [ADR 0050](0050-color-grade-film-grain-and-fxaa.md)).

## Decision

- `RendererConfig::sample_count`: 1 (off, the default) or 4. Anything else
  is clamped to 1 with a warning.
- Every scene pipeline (PBR, skinned, sky, skybox, ocean, terrain, grass,
  particles, billboards, 2D sprites) takes the sample count. The scene
  passes draw into MSAA colour and depth targets and resolve colour into the
  HDR buffer.
- Post-processing, shadow and blit passes stay single-sample. Depth is
  resolved by copying sample 0 (`depth_resolve.wgsl`, a pipeline made
  lazily) into the caller's depth view, so SSAO, fog, DoF and the other
  depth readers are unchanged. The opaque depth is resolved before the
  ocean's grab blit, which reads it, and again at the end so
  post-processing sees the whole frame's depth.
- The switch is `flint render --msaa 4` and `flint-player --msaa 4`. The
  default stays 1 so the headless pixel-diff gates stay single-sample.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- A new scene pipeline must take the sample count.
- FXAA stays available beside MSAA.
- Since then: MSAA is a quality setting (`QualitySettings::msaa`,
  FlintKart#55), set from `--quality`, the web player's `?msaa=` and
  `?quality=`, and scripts' `set_quality`, applied between frames:
  `set_msaa` rebuilds every scene pipeline against equal bind group layouts,
  so cached bind groups stay valid. Under `--msaa 4` the style mask is drawn
  4x and resolved so thin features keep their style (#306, PR #451).

## Sources

- Commit
  [`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)
  "Batch: F4 Rendering & Effects debug menu + Tier 2 renderer pass +
  prologue variants (ADRs [0053](0053-one-rendering-and-effects-menu-on-f4.md)-[0061](0061-flight-trails-from-script-driven-emitters.md))"
- Vault: "2026-11-07 Flint Four Samples Per Pixel GH-306"
- Code now: `crates/flint-render/src/scene_renderer/mod.rs`
  (`RendererConfig::sample_count`, `set_msaa`),
  `crates/flint-render/src/depth_resolve.wgsl`,
  `crates/flint-cli/src/commands/render.rs`, `crates/flint-player/src/main.rs`,
  `crates/flint-player/src/bin/flint_web.rs`
- Book: [Rendering](../book/src/concepts/rendering.md);
  [Post-Processing](../book/src/concepts/post-processing.md), "MSAA";
  [Quality Settings](../book/src/concepts/quality-settings.md)
