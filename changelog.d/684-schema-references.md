### Schemas check references: `entity` and `asset` fields, exact-length vectors, `flint schema --list` and a JSON Schema (ADR 0115, #684)
**What changed.** `flint validate` (and the player's load summary) now
reports two new errors: `unknown-entity` for a field or `parent` that names
no entity (`ik_two_bone.target`, `spline_mesh.spline`), and `missing-asset`,
with every path tried, for `model.asset`, `script.source`, `material` and
`sprite` textures and the spline, terrain and grass files. `""` means "not
set". `material.blend_mode`, `sprite.mode` and `particle_emitter.blend_mode`,
`.shape`, `.sort` are enums. `vec2`/`vec4` are exactly 2 and 4 numbers; a
`color` is 3 or 4. Schema files gain `entity`, `asset` (with `kind`) and
`table` with `fields`; `float`, `boolean`, `integer` and `entity_ref` still
load, with an `old-type-name` warning for your own files. `flint schema`
gains `--list`, `--format json` and `--format json-schema --output <file>`
(editor completion), and exits 1 for an unknown name.
[Schemas](docs/book/src/concepts/schemas.md) has the type list and codes.

**What to do.** Run `flint validate`: a reference it flags was already
dangling, so fix the name, add the file, or set `""` when a script fills it.
A `vec2`/`vec4` field of yours holding another count becomes
`{ type = "array", element = "f32" }` or `color`. Respell old type names when
warned.
