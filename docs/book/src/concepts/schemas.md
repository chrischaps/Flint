# Schemas

Schemas define the structure of your game world. They specify what components exist, what fields they contain, and how they bundle together into archetypes. Schemas are TOML files stored in the `schemas/` directory of your project.

## Component Schemas

A component schema defines a reusable data type. Components live in the `components/` folder of a schema directory. This one is from the layer the engine's examples are written in ([below](#the-examples-layer)):

```toml
# schemas/examples/components/door.toml
[component.door]
description = "A door that can connect spaces"

[component.door.fields]
style = { type = "enum", values = ["hinged", "sliding", "rotating"], default = "hinged" }
locked = { type = "bool", default = false }
open_angle = { type = "f32", default = 90.0, min = 0.0, max = 180.0 }
```

### Field Types

| Type | A value is | Example |
|------|------------|---------|
| `bool` | true or false | `true` |
| `i32`, `i64` | an integer | `42` |
| `f32`, `f64` | a number; an integer is taken | `3.14` |
| `string` | text | `"hello"` |
| `entity` | the name of another entity in the scene, **checked to exist**; `""` means "not set" | `"left_hand"` |
| `asset` | the name of a file of a `kind`, **checked through the project's asset search**; `""` means "not set" | `"barrel"` |
| `vec2`, `vec3`, `vec4` | exactly that many numbers (`vec3` also takes `{ x, y, z }`) | `[1.0, 2.0, 3.0]` |
| `color` | `[r, g, b]` or `[r, g, b, a]` | `[1.0, 0.8, 0.6]` |
| `transform` | a table of `position`, `rotation` and `scale` | see [Transform](#transform) |
| `enum` | one of `values` | `"hinged"` |
| `array` | a list of `element`, of any length (`string` if none is given) | `["a", "b"]` |
| `table` | a table of any shape; with `fields`, a table of those fields | `{ xz = 0.25 }` |

**One spelling per type.** These are the names to write. Four older spellings still load, so no schema file breaks, and each gets an `old-type-name` warning from `flint validate` that names the file and the field: `float` (write `f32`), `boolean` (`bool`), `int` and `integer` (`i32`), `entity_ref` (`entity`). The engine's own schema files use the one spelling throughout, and a test holds them to it. A name that is no type at all (`"integr"`) still loads, checked as a `string`, with an `unknown-type` warning.

**References.** An `entity` field and an `asset` field are strings, and `flint validate` follows them:

```toml
[component.turret.fields]
target = { type = "entity", default = "" }
mesh   = { type = "asset", kind = ["models", "specs"], unless = "runtime" }
shot   = { type = "asset", kind = "audio" }
```

- `entity`: the name must be an entity of the scene, after prefabs are expanded (a prefab writes `"${PREFIX}_hand"`). An empty string is "not set", for a reference a script fills while the game runs. An entity's own `parent = "..."` is checked the same way.
- `asset`: `kind` is one of the kinds `flint asset where` takes (`models`, `textures`, `audio`, `scripts`, `files`, ...; [Assets](assets.md) has the table), or a list of them when a name may be either: a model may be a file or a procgen spec of that name. The name goes through the same search the engine loads with, so the error lists every path tried. `unless` names a sibling `bool` field that turns the search off when it is `true` (`model.runtime`: a mesh code uploads has no file). An empty string is "not set". An asset the scene declares with an `asset_def` of the same `name` passes: `flint asset resolve` makes it.

Both are errors ([`unknown-entity`, `missing-asset`](#diagnostic-codes)), and the player says them at load as it says the rest.

**Tables and arrays.** A `table` with `fields` is checked like a component: each value against its field, a key it doesn't declare a warning. An array's `element` is a type name or a whole field definition, so an array can hold references or tables:

```toml
shear  = { type = "table", fields = { xy = "f32", xz = "f32" } }
points = { type = "array", element = "vec2" }
links  = { type = "array", element = { type = "table", fields = { to = "entity", weight = "f32" } } }
```

A problem inside one is named by its path: `transform.shear.xzz`, `links[1].to`.

### Field Constraints

Fields can include validation constraints:

```toml
open_angle = { type = "f32", default = 90.0, min = 0.0, max = 180.0 }
style = { type = "enum", values = ["hinged", "sliding"], required = true }
```

- `default` --- value used when not explicitly set. The scene loader fills them into any component the entity lists but leaves partially specified (`flint entity create` writes only the fields you give it, and leaves the rest to the loader) (see [Scenes: loading](scenes.md#loading-and-saving)). Scripts can therefore rely on a schema default existing at runtime.
- `min` / `max` --- numeric range bounds
- `required` --- the field must be written (defaults to false). An archetype default or the field's own `default` counts as written
- `values` --- valid options for enum types

A component schema with a name and **no fields** is open: the component is known and nothing in its table is checked. See [a component only scripts read](#a-component-only-scripts-read).

## Scene-level tables

A scene file has tables that belong to no entity: `[camera]`, `[environment]`, `[post_process]`, `[living]`, `[wooze]`. Each is declared the way a component is, as a `[table.<name>]` in a file under the schema directory's `scene/`, and takes the same fields, types and constraints:

```toml
# schemas/scene/camera.toml
[table.camera]
description = "A scene's [camera] table: the starting view."

[table.camera.fields]
projection = { type = "enum", values = ["perspective", "orthographic"], default = "perspective" }
position = { type = "vec3", description = "Where the camera is, [x, y, z]" }
fov = { type = "f32", description = "Field of view in degrees (perspective only)" }
```

`flint schema --list` names them under **Scene tables**, and `flint schema post_process` prints one. A `[[wooze.zone]]` entry is the `zone` field of `[table.wooze]`: an array of tables with fields of their own.

Every scene load and `flint validate` check them in the same pass as the entities ([Scene Diagnostics](#scene-diagnostics)):

```text
$ flint validate scenes/hall.scene.toml
scenes/hall.scene.toml: 1 error, 2 warnings

  error[bad-enum-value] [camera] projection: "ortho" is not a value of `projection`
      hint: allowed values: orthographic, perspective
  warning[unknown-table] [enviroment]: no scene table named `enviroment`, so nothing reads it
      hint: did you mean `environment`?
  warning[unknown-field] [post_process] bloom_intesity: `post_process` has no field `bloom_intesity`
      hint: did you mean `bloom_intensity`?
```

- A top-level table **no schema declares** is `unknown-table`, a warning: nothing reads it, and the scene plays as if it were not written. `--strict` makes it an error, as it does an unknown field.
- A key a declared table doesn't declare is `unknown-field`, with the nearest name.
- A value of the wrong type, out of range or outside an enum is an error, and an `entity` or `asset` field is looked up (`[environment] skybox` is an `asset` of kind `files`).

The tables the engine reads into a Rust struct (`[camera]`, `[environment]`, `[post_process]`, and `[living]` and `[wooze]`) are stricter about one thing: a value of the wrong TOML type there stops the file from parsing, as `toml-parse` with the line and column, before any schema is consulted.

### A table of your own

A game adds a scene-level setting without touching the engine. Declare it in the game's schema layer:

```toml
# schemas/scene/weather.toml
[table.weather]
description = "What the sky is doing in this scene."

[table.weather.fields]
rain = { type = "f32", min = 0.0, max = 1.0, default = 0.0 }
wind = { type = "vec3", description = "Metres a second, in world space" }
kind = { type = "enum", values = ["clear", "storm"], default = "clear" }
```

write it in a scene:

```toml
[weather]
rain = 0.7
wind = [3.0, 0.0, -1.0]
kind = "storm"
```

and read it from the game's Rust: [Player Extensions: Reading the scene's tables](../guides/player-extensions.md#reading-the-scenes-tables). The loader keeps every top-level table it has no field for, as written, so the table reaches the extension whether or not a schema declares it; the schema is what gets it checked, and gets `[wether]` and `rian = 0.7` named. Like a component, a table declared with no fields is open: known, and nothing in it checked. A later schema directory replaces a table of the same name whole.

`schemas/components/` holds only components: a `[table.*]` written there (or a `[component.*]` under `scene/`) is ignored, and `flint validate` says so (`ignored-schema-key`) with the directory it belongs in.

## Scene Diagnostics

Every scene is checked against the loaded schemas in one pass that never stops at the first problem. Each problem is a **diagnostic**: a severity, a stable code, the file, the entity, the component, the field (a dotted path such as `position[1]`), a message and, where there is one, a hint. A diagnostic about a file that is not there also lists every path the engine `tried`. One in a [scene-level table](#scene-level-tables) names the table (`[post_process] bloom_intesity`) in place of an entity and a component; in JSON it has a `table` key.

```
$ flint validate scenes/hall.scene.toml
scenes/hall.scene.toml: 4 errors, 1 warning

  error[unknown-archetype] entity 'hero' field 'archetype': no archetype named `playr`, so the entity gets no defaults
      hint: did you mean `player`?
  error[bad-type] entity 'hero' transform.scale: expected vec3, got string
  error[unknown-component] entity 'lever' component 'interactible': no component named `interactible`, so nothing checks it
      hint: did you mean `interactable`?
  error[bad-enum-value] entity 'pond' water_body.shape: "lak" is not a value of `shape`
      hint: did you mean `lake`? allowed values: lake, river
  warning[unknown-field] entity 'hero' transform.postion: `transform` has no field `postion`
      hint: did you mean `position`?
```

The same pass runs in three places:

- **`flint check [dir|scene]`** runs it over every scene of a project, with the checks of the project around the scenes (assets, files, styles, constraints), and exits 1 if any diagnostic is an error. See [The `check` Command](../cli-reference/overview.md#the-check-command).
- **`flint validate <scene>`** prints the whole list for one scene, errors first, and exits 1 if any is an error. `--format json` puts the list in a `diagnostics` array (`severity`, `code`, `file`, `entity`, `component`, `field`, `message`, `hint`, `tried`) beside the constraint `violations`. It needs no constraint files.
- **Every scene load** (`flint play`, `flint-player`, `flint render`, the viewer, a scene transition, a `load_chunk`) logs one summary line and the first five, as warnings, and carries on. A scene with mistakes still plays; `flint validate` is the gate to run before playing.

```
WARN scenes/hall.scene.toml: 4 errors, 1 warning (the full list: flint validate scenes/hall.scene.toml)
WARN   error[unknown-archetype] entity 'hero' field 'archetype': no archetype named `playr`, so the entity gets no defaults (did you mean `player`?)
```

### Diagnostic codes

The codes are stable: a code keeps its meaning, and a new kind of problem gets a new code.

| Code | Severity | Raised when |
|------|----------|-------------|
| `unknown-field` | warning | a component table has a field its schema does not declare; the hint is the nearest declared field |
| `bad-type` | error | a value of the wrong type for its field: a string where the schema says `f32`, a `vec3` of two numbers, a component that is not a table |
| `bad-enum-value` | error | a string that is none of an enum field's `values`; the hint lists them |
| `out-of-range` | error | a number below the field's `min` or above its `max` |
| `missing-field` | error | a `required` field that neither the entity, its archetype's defaults nor the field's `default` supplies |
| `unknown-component` | error | a component table that no loaded schema directory declares; the hint is the nearest component name |
| `unknown-archetype` | error | an `archetype = "..."` that no loaded schema directory declares, so the entity gets no defaults |
| `toml-parse` | error | the file is not valid TOML, or not a scene; the message has the line and column |
| `file-unreadable` | error | the scene file could not be read |
| `bad-prefab` | error | a `[prefabs.<name>]` instance whose template is missing, doesn't parse, or collides with an entity; the rest of the scene is still checked |
| `no-schemas` | warning | no schema is loaded at all, so nothing was checked (said once, not per entity) |
| `needs-component` | warning | a component that does nothing without another the entity lacks: a `collider` with no `rigidbody`. Raised [while the game runs](#diagnostics-while-the-game-runs), by the system that reads it |
| `unknown-entity` | error | an `entity` field, or an entity's `parent`, names no entity of the scene; the hint is the nearest name. An empty string is "not set" and passes |
| `missing-asset` | error | an `asset` field names no file the project's asset search finds; the hint lists every path tried |
| `old-type-name` | warning | a schema file of the game's spells a type the old way (`float` for `f32`); it still loads |
| `unknown-type` | warning | a schema file of the game's names a type that is none, an `asset` with no `kind`, or a `kind` that is no asset kind; the field is checked as a string |
| `ignored-schema-key` | warning | a schema file of the game's holds a key the loader ignores: a field written under `[component.x]` instead of `[component.x.fields]`, a misspelled key of a field, or a `[table.*]` in a component file |
| `unknown-table` | warning | a top-level table of the scene that no loaded schema directory declares as a [`[table.<name>]`](#scene-level-tables); the hint is the nearest table name. Not raised when no table schema is loaded at all |

`old-type-name`, `unknown-type` and `ignored-schema-key` are about a schema file, not the scene: `flint validate` and `flint check` print them with the file's name, for the game's own schema directories only. A schema file inside a Flint engine checkout (the engine's repository, or the `engine/` subtree a game carries) is the engine's, and is not reported.

`flint check` adds the codes of the project around the scene:

| Code | Severity | Raised when |
|------|----------|-------------|
| `missing-asset` | error | as above, and for every asset field `flint check` knows by kind (the asset catalog and a procgen spec asked first, as the player asks); `tried` lists the paths |
| `missing-file` | error | a file a scene, prefab, particle effect, glyph set, chunk, user style or script names by path is in none of the asset folders; `tried` lists the paths |
| `not-a-child` | error | an `ik_two_bone.tip` that is not a child of the entity naming it, so the chain is never solved |
| `scene-load` | error | the scene parses and the engine still cannot build it |
| `bad-spec` | error | the procgen spec a model or texture is made from does not parse |
| `bad-input-config` | error | the input config the scene or the manifest names does not parse |
| `bad-user-style` | error | a user style does not load or its shader does not compile (a warning for what the style's own check only warns about) |
| `bad-style-setting` | warning | `[post_process.styles]` names a style, a key or a value there is not |
| `bad-look` | error | the look a scene names is not found, or does not fit under the scene's keys |
| `bad-wooze` | error | the scene's `[wooze]` table, or the preset it names, is wrong |
| `bad-species` | error | a `[living.species.<spec>]` table is wrong, or no living prop grows from that spec |
| `constraint` | the constraint's | a [constraint](constraints.md) the scene breaks (`info` reads as a warning) |
| `bad-constraint` | error | a constraint file does not read, or a constraint's query does not parse or does not select entities |
| `bad-manifest` | error | `flint.toml` does not read |
| `bad-schemas` | error | a schema directory is missing, or a schema file does not read |

A component or archetype is known when **any** loaded schema directory declares it: the engine's, the game's, or one named by `--schemas`. Prefab instances are expanded first, so a prefab's entities are checked under the names they load with (`<prefix>_<name>`).

### Diagnostics while the game runs

The pass above checks a scene file against the schemas. A value can still be wrong where no schema looks: a component whose schema is open, a field a script writes with `set_field`, an entity a script spawns. So the systems that read components check as they read. Physics and audio read through a typed view of the component, and a value they cannot use is logged as a diagnostic of the same shape, then the default is used:

```
WARN error[bad-enum-value] entity 'crate' rigidbody.body_type: "dynamc" is not a value of `body_type` (did you mean `dynamic`? allowed values: static, dynamic, kinematic, kinematic_position, kinematic_velocity; the default "static" is used)
WARN error[bad-type] entity 'player' character_controller.jump_force: expected a number, got string "high" (the default 6 is used)
WARN warning[needs-component] entity 'wall' component 'collider': a collider with no rigidbody collides with nothing (add a `rigidbody` to the entity, `body_type = "static"` for scenery, or remove the collider)
```

Each (entity, component, field) is reported **once**, however many frames read it, and again after the scene is reloaded. The codes are `bad-type` and `bad-enum-value`, as at load, and `needs-component`. An integer where a number is read (`move_speed = 8`) is not a mistake and is not reported. These are log lines only: `flint validate` does not run the game, so it does not print them.

The components read this way are `rigidbody`, `collider`, `joint`, `character_controller`, `audio_source` and `audio_trigger`, and `material`'s `base_color_r/g/b/a`. A game's own Rust reads the same way: see [Player Extensions: Reading components](../guides/player-extensions.md#reading-components).

### Unknown fields and `strict`

An unknown field is a warning because games add fields to components and read them from scripts; an unknown scene table is one for the same reason (an extension may read it). To make both errors:

```bash
flint validate scenes/hall.scene.toml --strict     # for one run
```

```toml
# flint.toml: for every run in this project
[project]
strict = true
```

To silence the warning for a field that is meant to be there, declare it: put your own copy of the component file in the game's schema directory with the engine's fields and yours. A later directory replaces a component of the same name whole, so the copy must carry every field.

### A component only scripts read

A component that a scene writes and only a script reads still needs to be declared, or it is an `unknown-component`. Declare it by name in the game's schema directory. With fields it is checked; with none it is open:

```toml
# schemas/components/inventory.toml
[component.inventory]
description = "Read by scripts/inventory.rhai"
```

A component a script attaches at run time, which no scene file writes, is never seen by this pass and needs no schema.

Not checked by this pass: what only the reader of a table can know. `[post_process.styles.<name>]` is checked by the renderer against its style registry, a `[[wooze.zone]]`'s shape rules and `[living.species.<spec>]` by `flint validate`. The readers in the engine that fall back to a default on a wrong value are being moved to report through the same list.

## Built-in Components

Every component the engine reads has a schema in `schemas/components/`, and `flint schema <name>` prints it with each field's type, default and meaning. A test (`cargo test -p flint-schema --test engine_schemas`) keeps the two in step: every component constant in `flint_core::components` has a schema, and every schema is read by engine code. A few of them:

### Model and Light

The two components nearly every scene uses. `flint schema model` and `flint schema light` print the full text; [Lighting](lighting.md#the-light-component) has the light fields as a table.

```toml
# schemas/components/model.toml (descriptions cut short)
[component.model.fields]
asset = { type = "string", required = true }   # `barrel` is models/barrel.glb
runtime = { type = "bool" }                     # set by code that uploads a mesh

# schemas/components/light.toml
[component.light.fields]
type = { type = "enum", values = ["directional", "point", "spot"], default = "directional" }
color = { type = "vec3", default = [1.0, 1.0, 1.0] }
intensity = { type = "f32", default = 1.0 }
direction = { type = "vec3" }                   # directional: TOWARD the light
# range, radius, source_radius, inner_angle, outer_angle, angular_size,
# volumetric_intensity, volumetric_color
```

Most light fields carry no `default`. A default is written into the component when a scene loads, and some of these mean something by being absent: `range` falls back to the older `radius`, and `volumetric_color` follows `color`. Their fallbacks are in the field descriptions. `spline` and `spline_mesh` have schemas too ([Scenes: Splines](scenes.md#splines)).

### Transform

```toml
# schemas/components/transform.toml
[component.transform]
description = "Position and rotation in 3D space"

[component.transform.fields]
position = { type = "vec3", default = [0, 0, 0] }
rotation = { type = "vec3", default = [0, 0, 0] }
scale = { type = "vec3", default = [1, 1, 1] }
```

### Bounds

```toml
# schemas/components/bounds.toml
[component.bounds]
description = "Axis-aligned bounding box"

[component.bounds.fields]
min = { type = "vec3", default = [0, 0, 0] }
max = { type = "vec3", default = [10, 4, 10] }
```

### Material

```toml
# schemas/components/material.toml
[component.material]
description = "PBR material properties"

[component.material.fields]
texture = { type = "string", default = "", optional = true }
roughness = { type = "f32", default = 0.5, min = 0.0, max = 1.0 }
metallic = { type = "f32", default = 0.0, min = 0.0, max = 1.0 }
color = { type = "vec3", default = [1.0, 1.0, 1.0] }
emissive = { type = "vec3", default = [0.0, 0.0, 0.0] }
```

### Rigidbody

```toml
# schemas/components/rigidbody.toml
[component.rigidbody]
description = "Physics rigid body"

[component.rigidbody.fields]
body_type = { type = "enum", values = ["static", "dynamic", "kinematic"], default = "static" }
mass = { type = "f32", default = 1.0, min = 0.0 }
gravity_scale = { type = "f32", default = 1.0 }
```

### Collider

```toml
# schemas/components/collider.toml
[component.collider]
description = "Physics collision shape"

[component.collider.fields]
shape = { type = "enum", values = ["box", "sphere", "capsule"], default = "box" }
size = { type = "vec3", default = [1.0, 1.0, 1.0] }
friction = { type = "f32", default = 0.5, min = 0.0, max = 1.0 }
```

### Character Controller

```toml
# schemas/components/character_controller.toml
[component.character_controller]
description = "First-person character controller"

[component.character_controller.fields]
move_speed = { type = "f32", default = 5.0, min = 0.0 }
jump_force = { type = "f32", default = 7.0, min = 0.0 }
height = { type = "f32", default = 1.8, min = 0.1 }
radius = { type = "f32", default = 0.4, min = 0.1 }
camera_mode = { type = "enum", values = ["first_person", "orbit"], default = "first_person" }
```

### Sprite

```toml
# schemas/components/sprite.toml
[component.sprite]
description = "Billboard sprite rendered as a camera-facing quad"

[component.sprite.fields]
texture = { type = "string", default = "", description = "Sprite sheet texture name" }
width = { type = "f32", default = 1.0, min = 0.01, description = "World-space width" }
height = { type = "f32", default = 1.0, min = 0.01, description = "World-space height" }
frame = { type = "i32", default = 0, min = 0, description = "Current frame index" }
frames_x = { type = "i32", default = 1, min = 1, description = "Columns in sprite sheet" }
frames_y = { type = "i32", default = 1, min = 1, description = "Rows in sprite sheet" }
anchor_y = { type = "f32", default = 0.0, description = "Vertical anchor (0=bottom, 0.5=center)" }
fullbright = { type = "bool", default = true, description = "Bypass PBR lighting" }
visible = { type = "bool", default = true }
```

The sprite component is used for billboard sprites that always face the camera. See [Rendering: Billboard Sprites](rendering.md#billboard-sprites) for details on the rendering pipeline.

## Archetype Schemas

Archetypes bundle components together with defaults. They live in the `archetypes/` folder of a schema directory:

```toml
# schemas/examples/archetypes/wall.toml
[archetype.wall]
description = "A solid wall segment"
components = ["transform", "bounds"]

[archetype.wall.defaults.bounds]
min = [-5, 0, -0.15]
max = [5, 4, 0.15]
```

The `components` array lists which component schemas an entity of this archetype requires. The `defaults` section provides values used when a component field isn't explicitly set.

### Built-in Archetypes

The engine's own set, `schemas/archetypes/`:

| Archetype | Components | Description |
|-----------|------------|-------------|
| `character` | transform | A character or NPC |
| `player` | transform, collider, rigidbody, character_controller | Player-controlled character with physics and a first-person camera |
| `sprite` | transform, sprite | A flat 2D sprite |
| `animated_sprite` | transform, sprite, sprite_animator | A 2D sprite with sheet animation |
| `terrain` | transform, terrain | Heightmap terrain |

### The Examples Layer

The scenes under `examples/` are written in a room-builder vocabulary that is not part of the engine's set. It is a schema directory of its own, `schemas/examples/`:

| Archetype | Components | Description |
|-----------|------------|-------------|
| `room` | transform | A parent that groups a room's entities |
| `door` | transform, door, audio_trigger | A door |
| `furniture` | transform, bounds | A box, 1 m a side by default |
| `wall` | transform, bounds | A wall segment |
| `floor` | transform, bounds | A floor slab |
| `ceiling` | transform, bounds | A ceiling panel |
| `pillar` | transform, bounds | A column |

with the `door` component and three constraints about rooms and doors (`schemas/examples/constraints/basics.toml`). The Flint checkout's own `flint.toml` lists the layer after the engine's set, so every example resolves these names with no flag:

```toml
# flint.toml at the root of the Flint repository
[project]
schemas = ["schemas", "schemas/examples"]
```

A game gets none of them by default: `flint schema furniture` in a game finds nothing, and an entity with `archetype = "furniture"` takes no defaults from it and is an `unknown-archetype` error under `flint validate`. A game that wants the vocabulary (to follow [Your First Project](../getting-started/first-project.md), or to run an example scene of the engine's from inside the game) lists the layer between the engine's set and its own:

```toml
[project]
schemas = ["engine/schemas", "engine/schemas/examples", "schemas"]
```

Most games are better served by archetypes of their own. Every archetype with `bounds` draws a box, so `furniture` on an entity with no shape (a light, a script holder) draws a 1 m box where nothing was meant.

## Introspecting Schemas

Use the CLI to inspect schema definitions:

```bash
# Every component, scene table and archetype that is loaded
flint schema --list

# One component, one scene table, or one archetype
flint schema light
flint schema post_process
flint schema player

# From somewhere else, or to pick the directories yourself
flint schema light --schemas engine/schemas --schemas schemas
```

With no `--schemas`, `flint schema` reads the directories the project's `flint.toml` names, or `schemas/` in the working directory when there is no manifest ([Which directories load](#which-directories-load)).

`flint schema <name>` prints the fields, types, defaults and ranges; a table that declares fields of its own (a `[[wooze.zone]]`) has them listed beneath it. A name that is no component, scene table or archetype is an error (exit code 1) with the nearest name as a suggestion.

### Schemas as data

```bash
flint schema --list --json           # names, descriptions, each component's fields
flint schema light --format json     # one component or archetype
flint schema --format json           # everything: every field's type, default, range, values
```

`--json` is `--format json`. The whole export has `schema_dirs`, `types` (the type names above), `old_type_names`, `components`, `tables` (the [scene-level tables](#scene-level-tables)) and `archetypes`. A component or a table has `kind`, `description`, `open` (true when it declares no fields) and `fields`; a field has `type`, `required`, and where they apply `default`, `description`, `min`, `max`, `values` (an enum), `element` (an array), `kinds` and `unless` (an asset) and `fields` (a table).

### Editor completion: a JSON Schema for scenes

```bash
flint schema --format json-schema --output scene.schema.json
```

writes a JSON Schema (draft-07) for a `.scene.toml` under the schemas of your project: each entity takes `archetype` (one of the loaded archetypes), `parent`, and a table per component, with every field typed, described and given its default, range and enum values. [Taplo](https://taplo.tamasfe.dev/) and the Even Better TOML extension for VS Code read it, and give completion and inline errors while you type. Point a scene at it with a first line:

```toml
#:schema ../scene.schema.json

[scene]
name = "Hall"
```

or for every scene at once, in a `.taplo.toml` at the project root:

```toml
[[rule]]
include = ["**/*.scene.toml"]

[rule.schema]
path = "./scene.schema.json"
enabled = true
```

What it holds to, and what it leaves to `flint validate`:

- A table under an entity that is no loaded component is refused, as `flint validate` refuses it. A key a component doesn't declare is allowed, as the engine allows it (a warning).
- A field is `required` only when nothing else can supply it: it has no default and no archetype gives it one.
- Whether an `entity` or an `asset` leads anywhere is not something a JSON Schema can say: run `flint validate`.
- `[scene]`, `[prefabs.<name>]` and every [scene-level table](#scene-level-tables) a loaded schema declares (`[camera]`, `[post_process]`, a game's `[weather]`) are described, so their keys complete too. A top-level table none declares is allowed, as the engine allows it (a warning).

The file is generated from the schema directories, so write it again when a schema changes. `--output` writes plain UTF-8; a shell redirect on Windows may add a byte-order mark that JSON readers refuse. `flint schema light --format json-schema` is one component's table alone, and `flint schema post_process --format json-schema` one scene table's.

## Creating Custom Schemas

To add a new component:

1. Create a file in `schemas/components/`:

```toml
# schemas/components/health.toml
[component.health]
description = "Hit points and damage tracking"

[component.health.fields]
max_hp = { type = "i32", default = 100, min = 1 }
current_hp = { type = "i32", default = 100, min = 0 }
armor = { type = "f32", default = 0.0, min = 0.0, max = 1.0 }
```

2. Reference it in an archetype:

```toml
# schemas/archetypes/enemy.toml
[archetype.enemy]
description = "A hostile NPC"
components = ["transform", "health"]

[archetype.enemy.defaults.health]
max_hp = 50
current_hp = 50
```

3. Use it in a scene:

```toml
[entities.goblin]
archetype = "enemy"

[entities.goblin.transform]
position = [10, 0, 5]

[entities.goblin.health]
max_hp = 30
current_hp = 30
armor = 0.1
```

No engine recompilation needed --- schemas are loaded at runtime from the TOML files.

## Game Project Schemas

Games can define their own schemas that extend or override the engine's built-in schemas. Several directories load in order, with later ones taking priority. A game names them once in its `flint.toml`:

```toml
[project]
schemas = ["engine/schemas", "schemas"]
```

Here `engine/schemas/` provides the engine's built-in components (transform, material, rigidbody, etc.) and the game's own `schemas/` adds game-specific components (health, weapon, enemy AI). If both directories define a component with the same name, the game's definition wins.

### Which directories load

Every command that loads schemas, the viewer and the player ask one resolver, so they always agree:

1. the `--schemas` flags, if any were given (repeatable on every command; they are the whole list, in the order given);
2. else the `schemas` list in the `flint.toml` of the project the scene belongs to;
3. else `schemas/` in the working directory, beside the scene, and one level up from the scene, whichever exist.

```bash
# From anywhere in a game with the manifest above: no flags
flint render scenes/arena.scene.toml -o arena.png

# The flags override the manifest for one run
cargo run --manifest-path engine/Cargo.toml --bin flint-player -- \
  scenes/arena.scene.toml \
  --schemas engine/schemas \
  --schemas schemas
```

A directory named by a flag or by the manifest that doesn't exist is an error naming the directory and where the name came from (`schema directory not found: nope (named by --schemas)`). See [Building a Game Project](../guides/building-a-game-project.md#multi-schema-layering).

### Game Project Directory Structure

Game projects live in their own repositories with the engine included as a git subtree:

```
my_game/                    (standalone git repo)
├── engine/                 (git subtree ← Flint repo)
│   ├── crates/
│   ├── schemas/            (engine schemas)
│   └── Cargo.toml
├── schemas/
│   ├── components/
│   │   ├── health.toml
│   │   ├── weapon.toml
│   │   └── enemy_ai.toml
│   ├── archetypes/
│   │   ├── enemy.toml
│   │   └── pickup.toml
│   └── scene/              (the game's own scene-level tables)
│       └── weather.toml
├── scripts/
│   ├── enemy_ai.rhai
│   ├── weapon.rhai
│   └── hud.rhai
├── scenes/
│   └── level_1.scene.toml
├── sprites/
│   └── imp.png
└── audio/
    ├── shotgun.ogg
    └── imp_death.ogg
```

This separation keeps game-specific data out of the engine directory, allowing multiple games to share the same engine schemas while defining their own components and archetypes. See [Building a Game Project](../guides/building-a-game-project.md) for the full setup guide.

## Further Reading

- [Entities and ECS](entities-and-ecs.md) --- how schemas connect to the entity system
- [Constraints](constraints.md) --- rules that validate entities against schemas
- [Scenes](scenes.md) --- how schema-defined entities are serialized
- [Rendering](rendering.md) --- billboard sprite rendering pipeline
- [CLI Reference](../cli-reference/overview.md) --- multi-schema CLI usage
