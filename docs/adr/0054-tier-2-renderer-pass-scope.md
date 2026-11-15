# ADR 0054: Scope of the Tier 2 renderer pass (decision not recovered)

Status: accepted, 2026-08-20 (commit
[`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)).
Backfilled 2026-11-12 (#374) from commit `aac2d27`, the only source that
names it.
The original record was in the Starchild repo, which no longer exists; this
is a reconstruction.

## Context

After the clay-look Tier 1 levers ([ADR 0048](0048-clay-lighting-levers-oren-nayar-and-sheen.md)
to [ADR 0052](0052-authorable-ssao-sample-count.md)), a second renderer pass
followed. Commit `aac2d27` lists it as "ADRs 0054-0058: Tier 2 renderer
pass — scope doc, wgpu depth-convention fix (tech-debt #12), source-radius
area lights, PCSS directional cascades, MSAA CLI gate with sample-0 depth
resolve". Taken in order, ADR 0054 is the scope doc.

## Decision

The scope doc's text did not survive. What the pass delivered is recorded
in [ADR 0055](0055-wgpu-zero-to-one-depth-convention.md) (the `[0, 1]`
depth convention), [ADR 0056](0056-source-radius-area-lights.md) (area
lights), [ADR 0057](0057-pcss-contact-hardening-shadows.md) (PCSS) and
[ADR 0058](0058-opt-in-4x-msaa-for-scene-passes.md) (MSAA).

## Alternatives considered

Not recorded in the surviving text.

## Consequences

Nothing in the code or the book cites ADR 0054.

## Sources

- Commit
  [`aac2d27`](https://github.com/chrischaps/Flint/commit/aac2d27af7f7dc4239285f069b4839511f97dec2)
  "Batch: F4 Rendering & Effects debug menu + Tier 2 renderer pass +
  prologue variants (ADRs [0053](0053-one-rendering-and-effects-menu-on-f4.md)-[0061](0061-flight-trails-from-script-driven-emitters.md))"
