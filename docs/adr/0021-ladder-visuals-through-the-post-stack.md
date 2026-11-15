# ADR 0021: The ladder's visuals drive the engine post stack, over the authored look

Status: accepted, 2026-08-16 (commit
[`bd4eb73`](https://github.com/chrischaps/Flint/commit/bd4eb7329fe776cb61cc3471e66750401f25fefc)).
Backfilled 2026-11-12 (#374) from commit `bd4eb73`, the code that cites it,
the book and the Vault note "2026-08-16 Starchild F5 — the ash-grey rule". The
original record was in the Starchild repo, which no longer exists; this is a
reconstruction.

## Context

The ladder's visual half (`desaturate`, `blur`, `chromatic`;
[ADR 0015](0015-disintegration-ladder-as-data.md)) had been drawn only in the
`play-chart` window. In the player, F3 merged it into post overrides, but the
engine's post stack had no desaturation, and the merge had a bug: it wrote
only while a rung was active. The renderer's post config keeps whatever was
last written, so a world that recovered stayed degraded. With blur and
chromatic at low rungs that was hard to see; with desaturation it would have
been a permanently grey game (Vault).

## Decision

- **Desaturation in the engine.** A `desaturate` field runs through
  `PostProcessConfig` and its uniform (reusing a pad slot, with the size
  asserted), the composite shader, `PostProcessDef` and a `set_desaturation`
  Rhai setter. A `PostProcessOverrides` struct replaces a 7-tuple.
- **The ash-grey rule.** The shader mixes toward darkened grey,
  `mix(color, luma * 0.62, amount)` with Rec.601 weights, after tonemapping,
  matching the window harness's formula, which the feel gates signed off
  first. Never neutral grey: "neutral grey reads as a filter; ash reads as
  loss" (Vault).
- **Pinned mapping** from rung to post: `desaturate` ×1.0, `blur` ×0.6 as
  radial blur, `chromatic` ×1.0.
- **Always write.** `LadderPostBase` snapshots the scene's authored values at
  session start. Every frame the merge writes `base + rung × scale` into each
  slot a script has not overridden, and after teardown the base is restored
  once. Script overrides win; preroll leaves the world untouched.
- `flint render --desaturate` for evidence renders.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- A ladder that recovers to clean restores the authored look.
- The Vault note draws a general rule: when layering a temporary effect over
  authored state, own the return path, not just the entry.
- The 0.62 constant lives in two shaders, the harness and the engine's
  composite, and should stay identical in both.
- The scales are marked provisional in code, owned by the Phase 4 look check,
  and the direct path is a fallback: F4's conducted parameters move
  authorship into scripts ([ADR 0020](0020-conducted-parameters-script-surface.md)).
- Since then: the composite applies desaturation after the colour grade (ADR
  0050) and before render modes. The F4 menu's "Freeze script post
  overrides" switch stops these stamps while tuning.

## Sources

- Commit [`bd4eb73`](https://github.com/chrischaps/Flint/commit/bd4eb7329fe776cb61cc3471e66750401f25fefc)
  "F5: engine post-stack migration — desaturation end-to-end (ADR 0021)"
- Vault: "2026-08-16 Starchild F5 — the ash-grey rule"
- Code now: `crates/flint-player/src/player_app/music_session.rs`
  (`LadderPostBase`, `merge_ladder_postprocess`, `MUSIC_BLUR_SCALE`),
  `crates/flint-render/src/composite_shader.wgsl`,
  `crates/flint-script/src/api/post_process.rs` (`set_desaturation`),
  [Post-Processing](../book/src/concepts/post-processing.md),
  [Music Sessions](../book/src/concepts/music-sessions.md), "Post-Processing
  Integration"
