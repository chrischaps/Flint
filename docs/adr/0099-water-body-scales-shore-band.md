# ADR 0099: A water body scales the shoreline band

Status: accepted, 2026-10-20 ([#72](https://github.com/chrischaps/Flint/pull/72)).
Backfilled 2026-11-12 (#374) from PR #72, whose body holds the ADR text,
commit `261dd75`, and the Vault note "The Capitol - a water body's own shore
band (GH-230, ADR 0099)". Extends [ADR 0094](0094-water-on-any-mesh.md).

## Context

[ADR 0094](0094-water-on-any-mesh.md) left everything but tint, waves and flow to the `ocean` component,
the shoreline band ([ADR 0088](0088-shoreline-foam-from-scene-depth.md), [ADR 0091](0091-shore-foam-measured-across-the-water.md))
included: one `shore_foam_width` for the sea and every body. The band foams
wherever the bed lies within that depth of the surface, so a stream or a
shallow river foams white from bank to bank. The Capitol's wild beds are 3 to
30 cm deep and its canals' band is 14 cm, so the client drew its beds three
times as deep as the land it stands things on (TheCapitol#230).

## Decision

One per-body scale on the band's width: `WaterBodyLook::shore_foam`, and
`shore_foam` on the `water_body` component, clamped 0 to 4. 1 is the sea's
band (the default, and the sea's own); 0 turns it off.

- It rides the body uniform's spare `params.w`, so the layout is unchanged.
  `WaterBodyUniformsGpu::sea` sets it to 1.
- `ocean_shader.wgsl` scales `shore_foam_width` by it before the band and the
  shore search's radius. It's a uniform, so the branch stays in uniform
  control flow.
- Colour, strength, pace and reach stay the ocean's: scaling the width is
  enough to go from the sea's band to a hairline or none.

## Alternatives considered

Not recorded in the surviving text.

## Consequences

- A body can keep a fine band at its banks without a deeper bed. The default
  leaves every existing scene as it was.
- Strength isn't per body: a fainter band at the same width would need
  another uniform word.
- In The Capitol, rivers take 0.18, streams 0.08 and lakes 0.25, and their
  beds are drawn at the carve.
- `shore_foam` is read by `WaterBodyLook::from_component` but is not declared
  in `schemas/components/water_body.toml` (checked 2026-11-12).

## Sources

- [#72](https://github.com/chrischaps/Flint/pull/72) / commit
  [`261dd75`](https://github.com/chrischaps/Flint/commit/261dd7500a42e8ce6c66e7d1011dac7f603b9fc6)
  "Flint: a water body scales the shoreline band (ADR 0099)"
- Vault: "The Capitol - a water body's own shore band (GH-230, ADR 0099) - 2026-10-20"
- Code now: `crates/flint-render/src/water_body.rs` (`WaterBodyLook::shore_foam`),
  `crates/flint-render/src/ocean_shader.wgsl`
- Book: [Ocean](../book/src/concepts/ocean.md), "Water bodies"
