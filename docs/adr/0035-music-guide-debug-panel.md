# ADR 0035: A summonable Music Guide panel shows what the chart expects, behind `debug-hud`

Status: accepted, 2026-08-19 (commit
[`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)).
Backfilled 2026-11-12 (#374) from commits `86d21ec` and `aac2d27`, Vault
notes "2026-08-17 Legibility is the finding" and "2026-08-17 Manifest Map
timeline panel", and the code.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Playing World II, Chris couldn't tell what input it expected, and asked for
in-game cues or a debug UI. Star Child's rule is that no gamified UI ships.
The Vault note records the debug UI as the right immediate move, and the
request itself as the first playtest finding: the music was meant to
telegraph the verbs, and a player needing a guide says it didn't.

## Decision

- A "Music Guide" egui panel in `flint-player` (Phase 4 F8 slice), toggled
  with Backquote while a session runs. It lists upcoming pulse, press and
  flick windows within 8 beats with a countdown, and the per-channel targets
  beside the live stick and trigger state.
- It compiles only with the player's `debug-hud` feature and is registered
  closed.
- The data is a purpose-shaped snapshot, `GuideFrame` / `GuideWindow`, from
  `ChartSession`. It carries only what `ConductedFrame` doesn't, and never
  exposes the conductor or tempo map. Judgment-shaped data exists in the
  engine; only summonable dev surfaces render it.
- `Judge` reports whether a pulse window is consumed, so a hit window needs
  no more guidance; it un-consumes across a rewind like everything else.
- Folding the player's per-effect F-keys into one menu was deferred.

## Alternatives considered

- **In-game cues** instead of a debug panel. Chris offered either; the
  surviving text says the debug UI was chosen as the immediate move, not why
  cues were set aside beyond the no-gamified-UI rule.

## Consequences

- The Manifest Map ([ADR 0037](0037-manifest-map-timeline-panel.md)) followed the same pattern: a static map built
  once at open plus a cheap per-frame snapshot. The Vault calls `GuideFrame`
  the template for new debug surfaces.
- Since then: [ADR 0053](0053-one-rendering-and-effects-menu-on-f4.md) (commit
  [`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2))
  completed the deferred fold: the old F1 and F5-F12 per-effect keys became
  the F4 Rendering & Effects menu.

## Sources

- Commit
  [`86d21ec`](https://github.com/chrischaps/Flint/commit/86d21ec404a5cf822956fcb4bf04058c6a2e3fe5)
  "World II baseline (engine): full verb map, per-channel judgment,
  alternates, cues, debug panels"
- Commit
  [`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)
  "Batch: F4 Rendering & Effects debug menu + Tier 2 renderer pass +
  prologue variants (ADRs [0053](0053-one-rendering-and-effects-menu-on-f4.md)-[0061](0061-flight-trails-from-script-driven-emitters.md))"
- Vault: "2026-08-17 Legibility is the finding"
- Vault: "2026-08-17 Manifest Map timeline panel"
- Code now: `crates/flint-player/src/player_app/music_guide_panel.rs`
  (`MusicGuidePanel`), `crates/flint-music/src/chart_session.rs`
  (`GuideFrame`, `GuideWindow`), `crates/flint-music/src/judgment.rs`,
  [Music Sessions](../book/src/concepts/music-sessions.md), "Debug Surfaces"
