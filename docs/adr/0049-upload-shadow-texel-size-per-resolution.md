# ADR 0049: Upload the shadow-map texel size so the resolution is real

Status: accepted, 2026-08-19 (commit
[`95a260a`](https://github.com/chrischaps/Flint/commit/95a260a27da8ac5a2ceaed059c6af1ce2de8fb76)).
Backfilled 2026-11-12 (#374) from commit `95a260a`, commit `647cc41` and
the code and book lines that cite it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

One of the six clay-look Tier 1 levers in commit `95a260a` (ADRs
0048-0050), named there "the C3 shadow-texel fix". The shaders hardcoded a
shadow texel size of 1/2048. Any other `--shadow-resolution` filtered as if
the map were 2048 wide, so the flag did nothing useful. Its default was 1024,
and a `!= 1024` guard meant an explicit 1024 was ignored too.

## Decision

- The texel size (1 / resolution) is uploaded in `cascade_splits.w` of
  `ShadowUniforms`, which was padding.
- `w = 0` means unset: the shaders fall back to 1/2048, so a default or
  stale uniform keeps the old behaviour.
- The four PCF sites and two bias sites that hardcoded the size read the
  uploaded value.
- `--shadow-resolution` defaults to 2048, the renderer's construction size,
  and `flint render` rebuilds the shadow pass only when the flag differs.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- 512, 1024 and 4096 are real choices. The F4 menu's Shadows section
  rebuilds the pass when one is picked
  ([ADR 0053](0053-one-rendering-and-effects-menu-on-f4.md)).
- Since then: shadow resolution is also a quality setting
  (`QualitySettings::shadow_resolution`, FlintKart#55), and PCSS reads the
  same texel size for its kernel caps
  ([ADR 0057](0057-pcss-contact-hardening-shadows.md)).

## Sources

- Commit
  [`95a260a`](https://github.com/chrischaps/Flint/commit/95a260a27da8ac5a2ceaed059c6af1ce2de8fb76)
  "Clay-look Tier 1: all six gated levers (ADRs [0048](0048-clay-lighting-levers-oren-nayar-and-sheen.md)-[0050](0050-color-grade-film-grain-and-fxaa.md))"
- Commit
  [`647cc41`](https://github.com/chrischaps/Flint/commit/647cc416c22c96fb667c9b4bb60d9c884d155037)
  "docs: new pages for lighting, music sessions, procgen, debug panels,
  music CLI"
- Code now: `crates/flint-render/src/shadow.rs` (`ShadowUniforms::cascade_splits`),
  `crates/flint-render/src/lib.rs` (`shadow_uniforms_default_texel_is_unset`),
  `crates/flint-cli/src/commands/render.rs`
- Book: [Lighting](../book/src/concepts/lighting.md), "Shadows";
  [CLI overview](../book/src/cli-reference/overview.md)
