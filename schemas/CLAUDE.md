# schemas: the engine's schema set, layering, the examples' layer (moved out of the root CLAUDE.md by #992)

The root file keeps a short form of each bullet. How the directories are resolved in code (`project::schema_dirs`, ADR 0114): `crates/flint-core/CLAUDE.md`, "Project and manifest".

## Layering and resolution

- **Schemas in `schemas/`** -- `components/*.toml` for field types, `archetypes/*.toml` for bundles, `scene/*.toml` for scene-level tables (`[table.<name>]`: `[post_process]`, a game's own `[weather]`; #686, `crates/flint-core/CLAUDE.md`). Several directories load in order, later overriding earlier (game overrides engine): a later `[component.x]` replaces the earlier whole, unless it says `extend = true`, which adds its fields to it (#794; `SchemaRegistry::component_origin` says which file each field came from). `[archetype.x] extend = true` appends its components and merges its defaults key by key (#1054; `archetype_origin`). One resolver names them for every command, the viewer and the player (ADR 0114; `crates/flint-core/CLAUDE.md`): repeatable `--schemas` flags if given (the whole list), else `[project] schemas` in the project's `flint.toml`, else `schemas/` in the working directory, beside the scene and one level up
- **Game project pattern** -- games in own repos with engine as git subtree at `engine/`; `schemas = ["engine/schemas", "schemas"]` under `[project]` in the game's `flint.toml` layers the game's schemas over the engine's with no flags, and `--schemas engine/schemas --schemas schemas` overrides it for a run

## The examples' layer (#681)

- `schemas/` -- Engine component, archetype and scene-table TOML definitions; `schemas/examples/` -- the room-builder layer the examples are written in, not part of the engine's set
- `flint.toml` -- the checkout as a project: `schemas = ["schemas", "schemas/examples"]`, read by everything run from this tree (a game with the engine as a subtree never reads it)
- `furniture`, `wall`, `floor`, `ceiling`, `pillar`, `room`, `door` (and the `door` component) are not the engine's: they are the examples' layer, `schemas/examples/` (#681). The root `flint.toml` lists it after `schemas/`, so every scene in this checkout resolves them with no flag; a `--schemas` flag is the whole list, so `--schemas schemas` alone leaves them out (add `--schemas schemas/examples`), and a game has none of them unless its own `flint.toml` lists `engine/schemas/examples`. In an example, never put `furniture` on a non-visual entity: it includes `bounds` and draws a green box. A `bounds` box's colour is its archetype's `[archetype.<name>.visual]` (`color`, `wireframe`; #751), which each host hands the renderer with `SceneBinding::with_archetypes(&registry)`; the renderer names no archetype, and one with no `visual` draws grey

## Every component has a schema, every schema a reader (ADR 0115)

- Every `flint_core::components` constant has a schema in `schemas/components/`, and every schema a reader: `cargo test -p flint-schema --test engine_schemas` holds it (ADR 0115; what it checks: `crates/flint-core/CLAUDE.md`). A schema field's `default` is written into the component at load, so give one only where the reader falls back to that same value
