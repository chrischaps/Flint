### `[packages]` in `flint.toml`, `unlisted-package` and `flint package used` (#1334)
**What changed.** A game can list the engine packages it uses under
`[packages]` beside `[project]` (`living = "*"` for a feature package,
`debug-hud = false` to drop a standard one). With the table, only the
listed packages' schemas load and only their script modules are in `flint
script api`; a component, scene table or script function of a package left
out is an `unlisted-package` error in `flint check`, `flint validate` and
`flint script check`, whose hint is the line to add. A name no package has
is a `bad-manifest` naming the line, with the nearest name. `flint package
used [--write]` reads the scenes, prefabs and scripts and lists (or writes)
what they use. `flint init` writes an empty `[packages]` in every template.
**What to do.** Nothing: without a `[packages]` table every package is in,
as before. To list yours, run `flint package used --write` in the project.
A game binary that builds a `flint_core::project::SchemaDirs` field by field
adds `packages: None`.
