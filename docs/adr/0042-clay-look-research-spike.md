# ADR 0042: A clay-look research spike (decision not recovered)

Status: accepted, 2026-08-19 (commit
[`e3a4ff0`](https://github.com/chrischaps/Flint/commit/e3a4ff06816434c948ba038a0f2e77255e06fc27)).
Backfilled 2026-11-12 (#374) from commit `e3a4ff0`, Vault notes "Clay as a
Material for Feelings - 2026-08-19" and "2026-08-19 Clay Look Tier 0
Photography Pass", and the book's lighting chapter.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Chris asked whether Star Child could look like clay. The commit that
followed names ADRs 0042-0047 and lists, first, the "clay-look research
spike (scenes, generator, technique menu, Tier 0 photography pass)", then
the engine levers, then the viewer follow-ups. The levers and follow-ups
map to 0043-0047 in that order, which leaves this number for the spike.
That mapping is an inference; nothing cites 0042 on its own.

## Decision

The record's own text did not survive. What the sources say the spike was:

- Scenes in the game's `spikes/clay_look/` (a ball, a rounded cube and a pot
  on terracotta ground), a generator for them, and a technique menu
  (`docs/clay-look-technique-menu.md`) of ways to get the look.
- A Tier 0 pass: the six scene and content items on that menu, which the
  Vault describes as photography (vignette, bounce cards, a softbox, macro
  perspective) rather than more surface detail.
- The engine changes the spike needed are their own records ([ADRs 0043](0043-scene-authored-hemisphere-ambient.md),
  0044, 0045).

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- The Vault note says the two changes that mattered most were the ambient
  hemisphere ([ADR 0043](0043-scene-authored-hemisphere-ambient.md)) and diffuse wrap ([ADR 0044](0044-diffuse-wrap.md)): clay reads as clay
  because light seems to soak into it.
- The spike found that Flint's depth linearizer read about half the true
  camera distance (tech-debt #12), later fixed by [ADR 0055](0055-wgpu-zero-to-one-depth-convention.md).
- Its render evidence (`logs/clay-look/`, 72 MB) was left untracked pending
  Chris's review. The verdict on the look was his to make.
- The book says ADRs 0042-0052 grew the levers for the clay look; Oren-Nayar
  and sheen came later ([ADR 0048](0048-clay-lighting-levers-oren-nayar-and-sheen.md)).

## Sources

- Commit
  [`e3a4ff0`](https://github.com/chrischaps/Flint/commit/e3a4ff06816434c948ba038a0f2e77255e06fc27)
  "Clay-look spike + viewer post-process follow-ups (ADRs 0042-0047)"
- Vault: "Clay as a Material for Feelings - 2026-08-19"
- Vault: "2026-08-19 Clay Look Tier 0 Photography Pass"
- Vault: "2026-08-19 Viewer DoF Follow (F12)"
- Book: [Lighting](../book/src/concepts/lighting.md), "The clay look"
