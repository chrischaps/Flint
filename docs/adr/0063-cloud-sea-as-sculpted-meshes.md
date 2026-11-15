# ADR 0063: The prologue's cloud sea is sculpted meshes, not a volumetric renderer

Status: accepted, 2026-08-22 (Starchild; no Flint commit).
Backfilled 2026-11-12 (#374) from the Vault note "2026-08-22 Star Child —
sculpting a cloud sea without volumetrics", which names "ADR 0063 in the
repo". Nothing in Flint cites it, so linking the number to this decision
rests on that one line.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

Chris asked for the prologue's placeholder cloud slabs to become a bed of
cloud with cumulus puffs, matching a concept painting, and was open to
extending the engine. Flint has no cloud raymarcher. The note's survey found
a flaw in every soft route: translucent meshes (per-object sorting,
opaque shadows), particles (unlit sprites) and the sky component's FBM
(it replaces the authored skybox). The concept itself was painted opaque
cumulus.

## Decision

From the note: sculpture, not simulation. A game-side generator,
`tools/gen_cloud_models.py`, builds the sea as a soft-max field of seeded
hemispherical domes at three scales and the puffs as hierarchical budding
sphere clusters. They are lit by the clay-look levers already in the engine
([ADR 0048](0048-clay-lighting-levers-oren-nayar-and-sheen.md)). The engine
was not extended.

## Alternatives considered

Translucent meshes, particles and the sky component's FBM, each set aside
for the flaw named above.

## Consequences

- The note's lessons: fluffiness came from hierarchy (each lobe grows
  smaller lobes) and steeper crowns; big forms must not drift, only the
  puffs move.
- It found that `fog_height_enabled` could never change the image (the
  height term was clamped under distance fog), logged as tech-debt #14.
- The cloud generator later gained a z-periodic tile mode for the intro
  ([ADR 0065](0065-intro-treadmill-with-credits-in-fog.md)).

## Sources

- Vault: "2026-08-22 Star Child — sculpting a cloud sea without
  volumetrics"
