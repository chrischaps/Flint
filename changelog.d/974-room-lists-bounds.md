### The examples' `room` archetype lists `bounds` (#974)
**What changed.** `schemas/examples/archetypes/room.toml` said a room was a
"transform-only parent container" while the layer's `rooms_have_bounds`
constraint failed `flint check` on every room without `bounds`. The archetype
now lists `components = ["transform", "bounds"]` and calls a room the box of
its bounds, drawn as an outline. It gives no default box: an archetype's
`components` list is what `flint schema room` prints, and nothing at load
reads it, so scenes load and draw exactly as before (renders byte-identical).
`cargo test -p flint-constraint --test schema_files` now fails a
`required_component` constraint that asks an archetype for a component the
archetype doesn't list.
**What to do.** Nothing, if your rooms write their `bounds`. An entity that is
only a level's root (it holds walls, floors and lamps) should carry no
archetype, as the showcase levels' roots have since #804; a `room` without
`bounds` still fails `rooms_have_bounds`.
