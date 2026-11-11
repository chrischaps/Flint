# ADR 0115: One list of scene diagnostics, complete schemas, and typed component reads

Status: accepted, 2026-11-10 (#683, with #681, #684, #685, #686, #687; audit epic #670)

## Context

Components are untyped `toml::Value`s read with string keys, which is what
lets an agent write a game as text. The cost is that a mistake does nothing
and says nothing, at every layer (line numbers at `54f1f6ba`):

- **Schemas are incomplete.** `model`, `light`, `spline` and `spline_mesh`
  have no schema file, so `flint schema light` prints nothing and a
  `directional_light` table passes validation and is lit by defaults.
- **Validation is shallow.** `validate_component_data`
  (`crates/flint-schema/src/validation.rs:7-33`) returns the first error and
  allows unknown fields "for flexibility". `InvalidFieldType` and
  `InvalidEnumValue` (`crates/flint-core/src/error.rs:47-62`) carry no field
  name. The loader (`crates/flint-scene/src/loader.rs:163,177-181`) warns
  only, skips a component with no schema, and skips an unknown archetype in
  silence.
- **The type system is thin.** No entity reference (`entity_ref` is an alias
  for string that nothing checks), no asset path, vectors of any length,
  a `color` type no schema uses. Scene-level tables (`[post_process]`,
  `[camera]`, `[wooze]`) have no schema mechanism; `wooze.toml` poses as a
  component to borrow one.
- **Readers default silently.** An unknown `body_type` becomes a fixed body,
  an unknown `shape` a box, an unknown audio bus Sfx, an integer where a
  float is read falls back to the default (`.as_float()` appears about 80
  times in runtime crates, though `toml_util` exists).
- **`flint validate` is not a gate.** With no constraint files it returns
  before loading the scene (`validate.rs:112-123`). The real reference checks
  live inside `flint bundle`.

Open issues #339, #282 and #556 each cover a corner of this.

## Decision

1. **One diagnostics type, produced by one pass.**

   ```rust
   pub struct Diagnostic {
       pub severity: Severity,        // Error | Warning
       pub code: &'static str,        // "unknown-field", "bad-type", "unknown-archetype", ...
       pub file: Option<PathBuf>,
       pub entity: Option<String>,
       pub component: Option<String>,
       pub field: Option<String>,     // dotted path
       pub message: String,
       pub hint: Option<String>,      // "did you mean `light`?", allowed values
   }
   ```

   `flint_scene::diagnose(scene_file, registry, project) -> Vec<Diagnostic>`
   never stops early. The player prints the list at load (as warnings, once).
   `flint validate` / `flint check` fail on any `Error` and print the list as
   text or JSON. The viewer shows it in a panel. Codes are stable, so an agent
   can act on them and a project can allow a specific one.

2. **What is an error.** A wrong type, an unknown enum value, an entity
   reference to nothing, a missing asset, an unknown component name and an
   unknown archetype are errors. An **unknown field in a component that has a
   schema is a warning with a suggestion**, not an error: games add their own
   fields to engine components, and scripts read them (see the first decision
   below).

3. **Every engine component has a schema**, enforced by a test that walks
   `flint_core::components` and `schemas/components/`. `model`, `light`,
   `spline` and `spline_mesh` are written first (#681).

4. **The schema language gains what scenes get wrong most** (#684):
   `entity` (checked to exist), `asset` with a kind (checked through the
   resolver of ADR 0114), fixed-length `vec2`/`vec3`/`vec4`, array-form
   `color`, and nested field tables. One spelling per type (`f32`, not
   `float`), with the old spellings accepted and listed by a lint. A
   `[table.<name>]` schema kind covers scene-level tables (#686), and
   `flint schema --format json-schema` exports the lot for editors.

5. **Readers go through a typed view that reports** (#685).

   ```rust
   let c = world.view(id, comp::COLLIDER);
   let friction = c.f32("friction", 0.5);                    // int or float
   let shape = c.choice("shape", &["box", "sphere", "capsule"], "box");
   ```

   A wrong type or unknown choice warns once per (entity, field) through the
   same `Diagnostic` shape, then uses the default. `toml::Value::as_float`
   joins clippy's `disallowed_methods` in runtime crates. Physics and audio
   are ported first.

## Steps

One agent owns #683 and carries out step 2 under its one claim, a PR a
step (or several, where a step says so): `Part of #683` on each PR but the
last, `Closes #683` on the last. The ticket's note has the details. A step
that names another ticket is that ticket's work, and waits its turn through
the tickets' blocked-by links. The steps get no sub-issues.

1. Schemas for the four missing components and the constants test (#681).
   Independent of the rest; can start now.
2. `Diagnostic`, `diagnose`, field names in errors, unknown archetype and
   component, the registry kept on `PlayerApp` (#683).
3. Schema types and the JSON Schema export (#684); scene tables (#686).
4. The typed view, physics and audio first (#685).
5. `flint check` over a project, sharing `flint bundle`'s reference checks
   (#687; also needs ADR 0114).

## Alternatives considered

- **Typed Rust structs per component** (serde with `deny_unknown_fields`).
  The strongest checking, and the end of runtime-defined components, which
  is a founding design choice: games define archetypes in TOML with no Rust.
- **Validation only, no typed view.** Catches what schemas describe and
  leaves every reader's silent fallback in place; a component without a
  schema, or a field a script writes at run time, would still fail quietly.
- **Errors at load in the player.** A typo would stop the game starting.
  Wrong for iteration: the player should run and say what is wrong, and the
  gate should be the command an agent runs before playing.

## Consequences

- Existing scenes will produce diagnostics on first run: stale
  `directional_light` tables in about 20 demo scenes, misspelled fields that
  have been silently ignored. That is the point, and it needs a pass over
  `demo/` and a changelog entry for the game repos (#765).
- `flint validate` becomes slower by one scene load in projects where it
  used to return early.
- A game can still add fields to an engine component; it gets a warning it
  can silence by declaring the field in its own schema layer.

## Decisions

Chris accepted the recommendations on 2026-11-10:

1. **An unknown field is a warning with a suggestion.** `--strict`, and
   `[project] strict = true` in the manifest, make it an error.
2. **The player prints diagnostics on every run**, summarized to one line
   plus the first few. `flint validate` prints the full list.
3. **Old type spellings** (`float`, `boolean`) are rewritten in the engine's
   schema files and warned about in a game's.

## Step 4 as built (#685)

- The view is `flint_ecs::ComponentView`, from `FlintWorld::view(id,
  component)`. A missing field or component reads as the default and is not
  reported; a wrong type or unknown choice reads as the default and is.
  `opt_*` readers cover a field whose absence means something else.
- **Runtime reports reuse `bad-type` and `bad-enum-value`**, at the severity
  they are listed with, since they mean the same thing; they are logged as
  warnings, as a load's summary is. One new code, `needs-component`
  (warning), for a collider with no rigidbody.
- **Once per (entity name, component, field)**, kept on the world
  (`FlintWorld::read_reports`), so a reloaded scene is told again and a test
  can read the list. Names, not ids: ids differ run to run.
- **The lint lives in the wasm check** (`scripts/wasm-clippy/clippy.toml`),
  the one check that walks exactly the runtime crates at deny level. It does
  not see tests, the CLI, the viewer, or code compiled only off wasm; the
  native clippy job does not carry the rule, because it prints warnings
  without failing and would flag every test's assertion on a `toml::Value`.
- Ported: physics, audio, and the renderer's material tint. The readers in
  the other runtime crates are not on the view yet. They coerce integers
  through `toml_util` and stay silent about a wrong type.
- The scene pass (`flint validate`) does not raise `needs-component`: it is
  found by the system that reads the component, while the game runs.

## Step 3, scene tables, as built (#686)

- **The kind is `[table.<name>]` in `scene/*.toml`** of a schema directory,
  written as a component is, loaded by the same layered resolver, and held
  in the registry beside the components (`get_table`, `tables`). A table
  schema is a `ComponentSchema` under another name, so every field type,
  `validate_component`'s checks and `references` apply unchanged
  (`validate_table`). `[[wooze.zone]]` is the `zone` field of `[table.wooze]`
  and `wooze_zone` is gone; `schemas/components/` holds only components.
- **One new code, `unknown-table`, a warning.** Nothing reads a table no
  schema declares, so the scene plays as it did; `--strict` and `[project]
  strict` promote it with `unknown-field`, since both are the scene saying
  what no schema declares. An unknown field in a declared table is the
  existing `unknown-field`. Nothing is said about tables when the registry
  holds no table schema at all (a schema directory from before the kind).
- **`Diagnostic` gained `table`**, beside `entity` and `component`, and
  prints as `[post_process] bloom_intesity`. The alternative was to put the
  table's name in `component`, which would have made JSON readers treat a
  scene table as a component of no entity.
- **`SceneFile` keeps what it has no field for** in `tables`, as written,
  and writes it back. It still has `camera`, `environment` and
  `post_process` as typed fields: the ticket's direction (only `scene`,
  `prefabs`, `entities` and `tables`) would have moved three structs that
  every renderer host reads, for no check gained. Their raw text is kept
  beside them (`written`) so a key serde dropped is still diagnosed. A wrong
  TOML type in a typed table remains a `toml-parse` error with its line.
- **No `SceneTable { name, schema, parse }` registry in code.** The schema
  is the registration: a game declares `[table.weather]` and reads the raw
  table through `ExtensionCtx::scene_table` (or `scene_table_as::<T>`). A
  parse hook would be a second place to declare the same table.
- **`scene.version` and a migrations list are not done.** There is one
  migration (`fold_legacy_style_keys`) and no second to shape a list around;
  the version is still parsed and only printed.
- **Found on the first run over `examples/`** (warnings, pinned in
  `crates/flint-schema/tests/scene_tables.rs`): four 2D examples write
  `[camera] orthographic = true`, which nothing reads (the key is
  `projection = "orthographic"`), so they have been drawn in perspective;
  `rolling_meadow` writes `flint render`'s `distance`/`pitch`/`yaw` as
  camera keys; `campfire` has `[lights.*]` tables. No frozen fixture is
  flagged.

### The living tables leave flint-scene (#686, second half)

- **`[living]` and `[wooze]` are flint-procgen's.** Their types, the preset
  files, `WoozeDef::field` / `check` and the look commit moved to
  `flint_procgen::scene_tables` as they were (flint-scene's `wooze.rs`,
  `commit.rs` and 175 lines of `format.rs`), and `SceneFile` holds the two
  tables raw in `tables`, as it holds a game's. flint-procgen now depends on
  flint-scene and flint-schema; nothing in flint-scene's tree depended on
  flint-procgen, so there is no cycle. flint-debug-ui and flint-viewer gained
  flint-procgen for the types.
- **The loader no longer fills in a wooze preset.** That was the last thing
  `load_scene` knew about one feature. A host that plays or draws a scene
  calls `flint_procgen::settle_scene(&mut scene_file, path)` after
  `load_scene`; `PlayerApp::apply_scene_settings` does it too, so a game
  binary built on it is unchanged. A call per host, not a registry of
  parse hooks in flint-scene: a hook a test or a tool forgets to install
  would change what a scene means without a word, where a missing call is a
  missing line in the one place that reads the tables.
- **A table that doesn't fit its type still refuses the scene**, from
  `settle_scene` instead of from the file's parse, so the message names the
  file and the table (`grove.scene.toml: [living]: unknown field glide`)
  and no longer a line and column.
- What stayed in flint-scene: `SceneEdit`, `SceneScalar` and
  `SceneDocument::apply_edit`, which are about any table, and two
  formatting rules in the patcher keyed by the names `living` and `wooze`
  (a new `[living]` goes after `[wooze]`; an emptied
  `[living.species.<x>]` is dropped). They are strings, not types.
