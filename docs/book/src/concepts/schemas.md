# Schemas

> **Package:** `engine`, the kernel: every game has it ([Packages](packages.md)).

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
| `asset` | the name of a file of a `kind`, **checked through the project's asset search**, or one of its `builtins`; `""` means "not set" | `"barrel"` |
| `vec2`, `vec3`, `vec4` | exactly that many numbers (`vec3` also takes `{ x, y, z }`) | `[1.0, 2.0, 3.0]` |
| `color` | `[r, g, b]` or `[r, g, b, a]`, and nothing else: a `{ r, g, b }` table or a `0xRRGGBB` integer is `bad-type`, as no reader takes either, and the error writes the array for you | `[1.0, 0.8, 0.6]` |
| `transform` | a table of `position`, `rotation` and `scale` | see [Transform](#transform) |
| `enum` | one of `values` | `"hinged"` |
| `array` | a list of `element`, of any length (`string` if none is given) | `["a", "b"]` |
| `table` | a table of any shape; with `fields`, a table of those fields | `{ xz = 0.25 }` |
| `any` | any value: the key is known and documented, and its component's reader judges the form | `[0.4, 0.9]`, `{ start = 1.0, end = 0.0 }` |

**One spelling per type.** These are the names to write. Four older spellings still load, so no schema file breaks, and each gets an `old-type-name` warning from `flint validate` that names the file and the field: `float` (write `f32`), `boolean` (`bool`), `int` and `integer` (`i32`), `entity_ref` (`entity`). The engine's own schema files use the one spelling throughout, and a test holds them to it. A name that is no type at all (`"integr"`) still loads, checked as a `string`, with an `unknown-type` warning.

**References.** An `entity` field and an `asset` field are strings, and `flint validate` follows them:

```toml
[component.turret.fields]
target = { type = "entity", default = "" }
mesh   = { type = "asset", kind = ["models", "specs"], unless = "runtime" }
shot   = { type = "asset", kind = "audio" }
```

- `entity`: the name must be an entity of the scene, after prefabs are expanded (a prefab writes `"${PREFIX}_hand"`). An empty string is "not set", for a reference a script fills while the game runs. An entity's own `parent = "..."` is checked the same way.
- `asset`: `kind` is one of the kinds `flint asset where` takes (`models`, `textures`, `audio`, `scripts`, `files`, ...; [Assets](assets.md) has the table), or a list of them when a name may be either: a model may be a file or a procgen spec of that name. The name goes through the same search the engine loads with, so the error lists every path tried. `unless` names a sibling `bool` field that turns the search off when it is `true` (`model.runtime`: a mesh code uploads has no file). An empty string is "not set". An asset the scene declares with an `asset_def` of the same `name` passes: `flint asset resolve` makes it. So does a model or texture the project's [asset catalog](assets.md#asset-catalog) registers in `assets/*.asset.toml` with its content in the store, as the player loads it from there first. A particle effect (`kind = "particles"`) or a sequence (`kind = "sequences"`) is found by the `name` its file states, as the engine registers it, not by the file's stem: `effect = "campfire"` is any visible `particles/*.particles.toml` with `name = "campfire"` (or, stating none, the one called `campfire.particles.toml`).
- `builtins` lists the names an `asset` field's reader takes with no file, which are never looked for: the built-in palettes of `ascii_style.palette` (`amber`, `cga`, ...), the built-in glyph set `builtin` of `ascii_style.font` and `ui_text.font`, and `"none"` for a style's `paper_image` or `stroke_image` (draw the procedural paper, the drawn strokes). Any other name is searched for as a file, and a miss's hint names the nearest built-in: `palette = "ambr"` is a `missing-asset` error, "palette 'ambr' not found", whose hint asks whether you meant `amber` and lists the built-ins ahead of the paths tried. A file of a built-in's name still loads ahead of it, as the engine's search finds it first.

  ```toml
  palette = { type = "asset", kind = "palettes", builtins = ["cga", "amber", "mono"] }
  paper   = { type = "asset", kind = "papers", builtins = ["none"] }
  ```

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
- `panel` --- what the field is on its component's F3 debug panel: `"status"`, `"tune"` or `"trigger"`, or a table of those keys ([A Panel From a Schema](../guides/debug-panels.md#a-panel-from-a-schema)). Validation ignores it

A component schema with a name and **no fields** is open: the component is known and nothing in its table is checked. See [a component only scripts read](#a-component-only-scripts-read).

## Scene-level tables

A scene file has tables that belong to no entity: `[camera]`, `[environment]`, `[post_process]`, `[living]`, `[wooze]`, `[panels]`. Each is declared the way a component is, as a `[table.<name>]` in a file under the schema directory's `scene/`, and takes the same fields, types and constraints:

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

Every scene load and `flint validate` check them in the same pass as the entities ([Scene Diagnostics](#scene-diagnostics)). This scene has three mistakes in its tables:

```toml
[scene]
name = "Hall"

[camera]
projection = "ortho"
position = [0, 2, 8]

[enviroment]
ambient_sky = [0.2, 0.25, 0.3]

[post_process]
bloom_intesity = 0.4
```

```text
$ flint validate scenes/hall.scene.toml
scenes/hall.scene.toml: 1 error, 2 warnings

  error[bad-enum-value] 5:1 [camera] projection: "ortho" is not a value of `projection`
      hint: allowed values: orthographic, perspective
  warning[unknown-table] 8:2 [enviroment]: no scene table named `enviroment`, so nothing reads it
      hint: did you mean `environment`?
  warning[unknown-field] 12:1 [post_process] bloom_intesity: `post_process` has no field `bloom_intesity`
      hint: did you mean `bloom_intensity`?
```

Each one is placed by the line and column of the key it names (`5:1` is `projection`), or, for a whole table, of its name in the header (`8:2` is the `enviroment` in `[enviroment]`). A missing field is placed at its table's header. JSON has them as `line` and `column`.

- A top-level table **no schema declares** is `unknown-table`, a warning: nothing reads it, and the scene plays as if it were not written. `--strict` makes it an error, as it does an unknown field.
- A key a declared table doesn't declare is `unknown-field`, with the nearest name.
- A value of the wrong type, out of range or outside an enum is an error, and an `entity` or `asset` field is looked up (`[environment] skybox` is an `asset` of kind `files`).

The tables the engine reads into a Rust struct (`[camera]`, `[environment]`, `[post_process]`, and `[living]` and `[wooze]`) are stricter about one thing: a value of the wrong TOML type there stops the scene from loading. `flint validate` and `flint check` say it once, by field, as the schema pass found it, and add why the scene is refused:

```text
  error[bad-type] 6:1 [camera] fov: expected f32, got string
      hint: the scene does not load until the table fits
```

`[living]` and `[wooze]` also refuse a key they don't declare, so a misspelt key there is an `unknown-field` error with the same hint, and its line. Every such table is named, not only the first, and the rest of the scene is still checked. A command that loads the scene (`flint play`, `flint render`) stops with the parser's own message instead; for `[living]` and `[wooze]` the load's warning just before it has the line.

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

and read it from the game's Rust: [Player Extensions: Reading the scene's tables](../guides/player-extensions.md#reading-the-scenes-tables). The loader keeps every top-level table it has no field for, as written, so the table reaches the extension whether or not a schema declares it; the schema is what gets it checked, and gets `[wether]` and `rian = 0.7` named. Like a component, a table declared with no fields is open: known, and nothing in it checked. A later schema directory replaces a table of the same name whole, unless it says `extend = true`, which adds its fields to it, as for a component ([Adding a field to an engine component](#adding-a-field-to-an-engine-component)): `[table.post_process]` with `extend = true` gives the engine's `[post_process]` a key of the game's own.

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

The same pass runs in four places:

- **`flint check [dir|scene]`** runs it over every scene of a project, with the checks of the project around the scenes (assets, files, styles, constraints), and exits 1 if any diagnostic is an error. See [The `check` Command](../cli-reference/overview.md#the-check-command).
- **`flint validate <scene>`** prints the whole list for one scene, errors first, and exits 1 if any is an error. `--format json` puts the list in a `diagnostics` array (`severity`, `code`, `file`, `entity`, `component`, `field`, `message`, `hint`, `tried`) beside the constraint `violations`. It needs no constraint files.
- **`flint edit <scene>`** lists it in the status panel at the bottom of the window, errors first, with each hint under its row, and lists it again on every hot reload. A row that names an entity is a link: clicking it selects the entity in the scene tree and the viewport. When an edit leaves the file unloadable (no longer TOML, say), the last good scene stays on screen and the panel shows the `toml-parse` error with its line. The panel is `flint validate`'s list, so `[project] strict` and the project's [allowed codes](#allowing-a-code) apply to it as they do there.
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
| `out-of-range` | error | a number below the field's `min` or above its `max`; a vector's or colour's members each (#492) |
| `missing-field` | error | a `required` field that neither the entity, its archetype's defaults nor the field's `default` supplies |
| `unknown-component` | error | a component table that no loaded schema directory declares; the hint is the nearest component name |
| `unknown-archetype` | error | an `archetype = "..."` that no loaded schema directory declares, so the entity gets no defaults |
| `toml-parse` | error | the file is not valid TOML, or not a scene; the message has the line and column |
| `file-unreadable` | error | the scene file could not be read |
| `bad-prefab` | error | a `[prefabs.<name>]` instance whose template is missing, doesn't parse, or collides with an entity; the rest of the scene is still checked |
| `no-schemas` | warning | no schema is loaded at all, so nothing was checked (said once, not per entity) |
| `needs-component` | warning | a component that does nothing without another the entity lacks: a `collider` with no `rigidbody` (written or from its archetype; a terrain or spline mesh makes its own body). Raised by `flint validate` and at load, and again [while the game runs](#diagnostics-while-the-game-runs) by the system that reads it |
| `second-player` | warning | a `character_controller` (written or from its archetype) on an entity that is not the player. The player is the first entity by name carrying one; the others' controllers are never read, and each is told which entity is the player. Raised by `flint validate`, `flint check` and at load |
| `unknown-entity` | error | an `entity` field, or an entity's `parent`, names no entity of the scene; the hint is the nearest name. An empty string is "not set" and passes |
| `parent-cycle` | error | the entities' `parent`s close into a loop: one parented to itself, or `a` under `b` under `a`. Said once a loop, on its first entity by name, with the chain; the scene does not load |
| `missing-asset` | error | an `asset` field names no file the project's asset search finds; the hint lists every path tried |
| `old-type-name` | warning | a schema file of the game's spells a type the old way (`float` for `f32`); it still loads |
| `unknown-type` | warning | a schema file of the game's names a type that is none, an `asset` with no `kind`, or a `kind` that is no asset kind; the field is checked as a string |
| `ignored-schema-key` | warning | a schema file of the game's holds a key the loader ignores: a field written under `[component.x]` instead of `[component.x.fields]`, a misspelled key of a field, or a `[table.*]` in a component file |
| `bad-panel` | warning | a component schema's fields say something about its [F3 panel](../guides/debug-panels.md#a-panel-from-a-schema) the loader can't use (a role that is none, a key a field's panel doesn't hold, a `format` with no placeholder); that row is left off the panel |
| `unknown-table` | warning | a top-level table of the scene that no loaded schema directory declares as a [`[table.<name>]`](#scene-level-tables); the hint is the nearest table name. Not raised when no table schema is loaded at all |
| `post-range` | warning | a `[post_process]` value outside the range its built-in effect is tuned for (`flint render --list-effects` lists every key with its range); it still draws, as a style value past its range does |
| `light-cap` | warning | a light past the number of its type the shader takes (4 directional, 16 point, 8 spot): the strongest directional lights and the point and spot lights nearest the camera are drawn, and this one is off while it isn't among them. Raised [while the game runs](#diagnostics-while-the-game-runs), by the renderer ([Lighting](lighting.md#when-a-light-cant-be-drawn)) |
| `two-forms` | warning | a value written in two forms of which one is read, such as a spot's `inner_angle` (radians) beside `inner_angle_deg`; the degrees win. Raised while the game runs |
| `shadow-cap` | warning | a light's `cast_shadows` the renderer can't honour: a second spot asking (one spot casts, the first by name), a point light asking (none casts yet), or the strongest directional light saying `false` (it casts the cascades whatever it says). The light still lights. Raised while the game runs ([Lighting](lighting.md#spot-light-shadows)) |

`old-type-name`, `unknown-type`, `ignored-schema-key` and `bad-panel` are about a schema file, not the scene: `flint validate` and `flint check` print them with the file's name, for the game's own schema directories only. The player says them too, once, when it first reads the schemas (`flint play` and `flint-player` at start, and a transition only for a directory it had not read), as the summary a scene load prints:

```text
WARN schemas/components/lantern.toml: 1 warning (the full list: flint check)
WARN   warning[old-type-name] lantern.brightness: `float` is an old spelling of `f32` (write type = "f32"; `float` still loads)
```

A schema file inside a Flint engine checkout (the engine's repository, or the `engine/` subtree a game carries) is the engine's, and is not reported.

`flint check` adds the codes of the project around the scene:

| Code | Severity | Raised when |
|------|----------|-------------|
| `missing-asset` | error | as above, and for every asset field `flint check` knows by kind (the asset catalog and a procgen spec asked first, as the player asks); `tried` lists the paths |
| `missing-file` | error | a file a scene, prefab, particle effect, glyph set, chunk, user style or script names by path is in none of the asset folders; `tried` lists the paths |
| `not-a-child` | error | an `ik_two_bone.tip` that is not a child of the entity naming it, so the chain is never solved |
| `scene-load` | error | the scene parses and the engine still cannot build it |
| `bad-spec` | error | the procgen spec a model or texture is made from does not parse |
| `bad-ui` | error | a UI document (`.ui.toml`) or the style file it names has an unknown type, anchor, key, parent, class or style property, or a value of the wrong shape, named with its element or class; `load_ui` refuses the same document ([Scripting](scripting.md#file-format-uitoml)) |
| `spec-param` | error or warning | a parameter of that spec, named by its path (`ops[2].columns`) as the field: an error for a value its generator can't use (wrong type, out of range, not one of the choices or a colour, a required key missing), a warning for a key it ignores (unknown, for another mode, deprecated); the file is the spec's |
| `bad-spline` | error | a `.spline.toml` a scene's `spline.file` names does not parse, placed by line and column; at load, also a `spline_mesh.profile` that is no polygon |
| `bad-emitter` | error | an inline `particle_emitter` has a key the emitter doesn't read or a value it can't use (a curve or range of the wrong form, a zero lifetime, an unknown shape), named by entity and key; it replaces the schema's `unknown-field`, `bad-type` and `bad-enum-value` for that component, so one mistake is said once, in the emitter's words (the schema declares every key the emitter reads, its many-form keys as `any`) |
| `bad-input-config` | error | the input config the scene or the manifest names does not parse |
| `bad-user-style` | error | a user style does not load or its shader does not compile (a warning for what the style's own check only warns about) |
| `bad-style-setting` | warning | `[post_process.styles]` names a style, a key or a value there is not; in a look, a style no folder that can take the look has (a game's own Rust style is known only to its binary, so this is a warning there too) |
| `bad-look` | error | the look a scene names is not found, or does not fit under the scene's keys; or a look file the project holds does not read |
| `bad-wooze` | error | the scene's `[wooze]` table, or the preset it names, is wrong |
| `bad-species` | error | a `[living.species.<spec>]` table is wrong, or no living prop grows from that spec |
| `bad-field` | error | a field the scene declares (`[field.<name>]`) has a key or zone that is wrong, named by the field and the key (`weather.keys.wind`) or zone (`weather.zone.storm`) as the field; the player warns and plays without the part that's wrong ([Scripting](scripting.md#fields-a-scene-declares)) |
| `constraint` | the constraint's | a [constraint](constraints.md) the scene breaks (`info` reads as a warning) |
| `bad-constraint` | error | a constraint file does not read, or a constraint's query does not parse or does not select entities |
| `bad-manifest` | error | `flint.toml` does not read, or a line of its `[packages]` names no package or leaves out one that can't be ([Packages a game uses](../guides/building-a-game-project.md#packages-a-game-uses)) |
| `bad-package` | error | a package's `package.toml` does not read, or provides what another package's does ([The `package` Command](../cli-reference/overview.md#the-package-command)) |
| `bad-schemas` | error | a schema directory is missing, or a schema file does not read |
| `unlisted-package` | error | a component, archetype, scene table or script function of a package the project's `[packages]` leaves out; the hint is the line to add (`living = "*"`) ([Packages a game uses](../guides/building-a-game-project.md#packages-a-game-uses)) |
| `unbuilt-package` | error | `flint check`: the project's `[packages]` lists a package this `flint` was built without; the hint is `flint build`, which builds a pair with it ([Building the game](../guides/building-a-game-project.md#building-the-game-flint-build)) |
| `missing-cargo-patch` | warning | a game's Cargo workspace root lacks a `[patch.crates-io]` line of the engine it builds by path, or points it elsewhere; the hint is the line to add ([Upgrading the Engine](../guides/upgrading-the-engine.md#cargo)) |
| `stale-cargo-lock` | warning | a game's `Cargo.lock` gives gpu-allocator a different `windows` crate from wgpu-hal's (a lock moved forward from before wgpu 30), so wgpu-hal fails on Windows with ``expected `ID3D12Device`, found a different `ID3D12Device` ``; the hint is the `cargo update` that mends it ([Upgrading the Engine](../guides/upgrading-the-engine.md#cargo)) |

The scripts pass, which `flint check`, `flint validate` and [`flint script check`](scripting.md#checking-your-scripts) run, adds the codes of a script, each at a line and a column (`line` and `column` in the JSON):

| Code | Severity | Raised when |
|------|----------|-------------|
| `script-syntax` | error | the script does not compile |
| `unknown-function` | error | a call names no script function, Rhai built-in, function of the file or of an imported module; the hint is the nearest name |
| `wrong-arity` | error | the function is there, at other argument counts; the hint lists its declared signatures |
| `bad-argument` | error | a literal argument no declaration of the function accepts: a string where a number goes, `0.5` for an `int` |
| `unknown-callback` | error | an `fn` one edit from a callback (`on_colision`), which nothing in the file calls: the engine never would |
| `callback-arity` | error | a callback defined with a parameter count the engine does not call it with |
| `missing-module` | error | `import "name"` finds no `scripts/lib/name.rhai`; `tried` lists the paths |
| `deprecated-function` | warning | a call of a function the API marks deprecated; the hint says what to use |
| `mixed-numbers` | warning | a call in `on_update` or `on_draw_ui` (or a function of the file they call) of a function with four or more number parameters, whose literal arguments mix ints and floats: the slow lookup |
| `unknown-global` | error | a `global::NAME` read in a script with no top-level `const NAME`: `global::` reaches constants only, so the read fails every time it runs. The hint names a top-level `let` of the name, or the nearest constant |

A component or archetype is known when **any** loaded schema directory declares it: the engine's, the game's, or one named by `--schemas`. Prefab instances are expanded first, so a prefab's entities are checked under the names they load with (`<prefix>_<name>`).

### Diagnostics while the game runs

The pass above checks a scene file against the schemas. A value can still be wrong where no schema looks: a component whose schema is open, a field a script writes with `set_field`, an entity a script spawns. So the systems that read components check as they read. Physics, audio, animation (`animator`, its `layers`, `sprite_animator`, `ik_two_bone`), the renderer (`sprite`, `material`, `bounds`, `model`, `material_overrides`, `terrain` and its grass, `grass_ground`, `sky`, `ocean` (its look and its waves), `water_body`, `spline`, `spline_mesh` and the 2D UI's `screen_anchor`, `parallax`, `ui_fill`, `ui_text`) the script system (`script`, `interactable`, `joint` for `get_joint_target`, the sprite and ocean functions) and the player (`camera_tuning`, `bone_probe` and the F3 panels' starting values) read through a typed view of the component, and a value they cannot use is logged as a diagnostic of the same shape, then the default is used:

```
WARN error[bad-enum-value] entity 'crate' rigidbody.body_type: "dynamc" is not a value of `body_type` (did you mean `dynamic`? allowed values: static, dynamic, kinematic, kinematic_position, kinematic_velocity; the default "static" is used)
WARN error[bad-type] entity 'player' character_controller.jump_force: expected a number, got string "high" (the default 6 is used)
WARN warning[needs-component] entity 'wall' component 'collider': a collider with no rigidbody collides with nothing (add a `rigidbody` to the entity, `body_type = "static"` for scenery, or remove the collider)
WARN error[bad-enum-value] entity 'hero' animator.layers[1].mode: "overide" is not a value of `mode` (did you mean `override`? allowed values: additive, override, replace; the layer is additive)
```

An entry of an array of tables is named by its index (`animator.layers[1].mode`), and a table inside a component by its key (`material_overrides.Glass.emissive`).

Each (entity, component, field) is reported **once**, however many frames read it, and again after the scene is reloaded. The codes are `bad-type` and `bad-enum-value`, as at load, and `needs-component`. An integer where a number is read (`move_speed = 8`) is not a mistake and is not reported. These are log lines only: `flint validate` does not run the game, so it does not print them. The one it can see without running, a collider on an entity with no rigidbody, it reports before the game starts as well.

The components read this way are `rigidbody`, `collider`, `joint`, `character_controller`, `audio_source`, `audio_trigger` and `light` (#682: an unknown `type`, a missing one, a spot angle in the wrong unit, a light past the shader's cap), and `material`'s `base_color_r/g/b/a`. A game's own Rust reads the same way: see [Player Extensions: Reading components](../guides/player-extensions.md#reading-components).

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

`strict` is a gate for the commands that pass or fail a scene: `flint validate`, `flint check` and the viewer's panel apply it. A **load does not**: `flint play`, `flint-player`, `flint render` and a scene transition log an unknown field as the warning it is raised as, `strict` or not, since a load never fails on what `strict` promotes and the scene plays the same either way.

To silence the warning for a field that is meant to be there, declare it in the game's schema directory as an extension of the engine's component: see [Adding a field to an engine component](#adding-a-field-to-an-engine-component). Or allow it, below.

### Allowing a code

A project can accept a diagnostic by its code, everywhere or at one place, in `flint.toml`:

```toml
[project.diagnostics]
allow = [
  "unknown-field:material.wetness",   # one field of one component (and what is nested under it)
  "unknown-field:weather",            # every unknown field of the `weather` component or scene table
  "unknown-table",                    # the code everywhere
]
```

An entry is a code from the table above, then optionally `:` and a place: a component or scene-table name, then `.` and a field's dotted path. A place covers what is nested under it, so `environment.fog` allows `environment.fog.colour`, and `material.wet` does not allow `material.wetness`. An allowed diagnostic is left out of what `flint validate`, `flint check`, the viewer's panel and every load's summary report, and each says how many it left out:

```
$ flint validate scenes/road.scene.toml
scenes/road.scene.toml: 1 warning (2 allowed by flint.toml)
```

With `--format json` (and `flint check --json`) the count is `allowed`. An allowed error no longer fails the scene. `flint bundle` reads the list from the `flint.toml` it packs and leaves out the same diagnostics `flint check` does, so a bundle refuses only what the check reports; it counts them too (`check: no problems (1 allowed by flint.toml)`, and `allowed` in `flint --json bundle`). A code that is none of the table's is refused when the manifest is read, with the nearest code (`project.diagnostics.allow[0]`: `unknown-feild` is no diagnostic code (did you mean `unknown-field`?)), and so are the codes that mean the file did not load at all (`toml-parse`, `file-unreadable`, `scene-load`, `bad-manifest`, `bad-package`, `bad-prefab`), since nothing behind them was checked.

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

Every component the engine reads has a schema in `schemas/components/`, or in its package's `packages/<name>/schemas/components/` ([above](#the-packages-schemas-come-with-the-engines)), and `flint schema <name>` prints it with each field's type, default and meaning. A test (`cargo test -p flint-schema --test engine_schemas`) keeps the two in step: every component constant in `flint_core::components` has a schema, and every schema is read by engine code. A few of them:

### Model and Light

The two components nearly every scene uses. `flint schema model` and `flint schema light` print the full text; [Lighting](lighting.md#the-light-component) has the light fields as a table.

```toml
# schemas/components/model.toml (descriptions cut short)
[component.model.fields]
asset = { type = "string", required = true }   # `barrel` is models/barrel.glb
runtime = { type = "bool" }                     # set by code that uploads a mesh

# schemas/components/light.toml
[component.light.fields]
type = { type = "enum", values = ["directional", "point", "spot"] }
color = { type = "vec3", default = [1.0, 1.0, 1.0] }
intensity = { type = "f32", default = 1.0 }
direction = { type = "vec3" }                   # directional: TOWARD the light
# range, radius, source_radius, inner_angle_deg, outer_angle_deg,
# inner_angle, outer_angle, angular_size, volumetric_intensity, volumetric_color
```

Most light fields carry no `default`. A default is written into the component when a scene loads, and some of these mean something by being absent: `range` falls back to the older `radius`, `volumetric_color` follows `color`, `direction` follows the entity's rotation, and a missing `type` reaches the renderer, which says so ([Lighting](lighting.md#when-a-light-cant-be-drawn)). Their fallbacks are in the field descriptions. `spline` and `spline_mesh` have schemas too ([Scenes: Splines](scenes.md#splines)).

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
height = { type = "f32", min = 0.1 }

# Fields taken out, each with what a scene still writing one is told (#1212)
[component.character_controller.retired]
radius = "nothing read it: the capsule is the entity's `collider` (...); remove it"
camera_mode = "nothing read it: a character's camera is always first person (...); remove it"
```

A `[component.<name>.retired]` table names fields a component no longer has. A scene that still writes one gets `unknown-field`, as for any field the schema doesn't declare, but the message ends `any more` and the hint is the reason written there instead of the nearest field's name. An extension's `retired` adds to the declaring layer's.

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

### An Archetype's Placeholder Box

An entity with a `bounds` box and nothing else to draw (no model, sprite or procedural shape) is drawn as that box. Its colour comes from its archetype's `visual` table:

```toml
# schemas/archetypes/crate.toml in a game
[archetype.crate]
components = ["transform", "bounds"]

[archetype.crate.visual]
color = [0.8, 0.55, 0.3]   # [r, g, b], or [r, g, b, a]: alpha under 1 is see-through
wireframe = false          # true draws only the box's edges
```

An archetype with no `visual`, and an entity with no archetype, draws grey (`[0.5, 0.5, 0.5]`). The renderer knows no archetype by name: the colours the examples' `wall`, `room` and the rest draw in are in their own files under `schemas/examples/archetypes/`, and `character`'s is in `schemas/archetypes/character.toml`. An entity's own `material.color` still colours its box over the archetype's. A key in `visual` other than `color` and `wireframe`, or a colour that isn't three or four numbers, fails the schema load with the file named. `flint schema <archetype>` prints the table as `Placeholder:`, and the JSON export (`--format json`) has it as `visual`.

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

The scenes in [flint-examples](https://github.com/chrischaps/flint-examples) are written in a room-builder vocabulary that is not part of the engine's set. It is a schema directory of its own, `schemas/examples/`:

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

Most games are better served by archetypes of their own, each with its [placeholder colour](#an-archetypes-placeholder-box). Every archetype with `bounds` draws a box, so `furniture` on an entity with no shape (a light, a script holder) draws a 1 m green box where nothing was meant.

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

`flint schema <name>` prints the fields, types, defaults and ranges; a table that declares fields of its own (a `[[wooze.zone]]`) has them listed beneath it. It names the file the component was declared in, and when a later layer [extended](#adding-a-field-to-an-engine-component) it, numbers the files and ends each field's line with the number of the one that declared it. A component whose fields put it on an [F3 panel](../guides/debug-panels.md#a-panel-from-a-schema) has a line for it under the header, its title and each role's fields in the panel's order, with a label where the panel calls a field something other than its name read as words (`end_now`'s "End now" is not repeated): `Panel "Dead calm": status phase, calm, next_in_s · trigger trigger_calm "Calm now", end_now`. An archetype prints its components and defaults the same way: the file that declared it, and when a later layer [extended](#adding-a-component-to-an-engine-archetype) it, the numbered files with each component and each default ending in its file's number. A name that is no component, scene table or archetype is an error (exit code 1) with the nearest name as a suggestion.

### Schemas as data

```bash
flint schema --list --json           # names, descriptions, each component's fields
flint schema light --format json     # one component or archetype
flint schema --format json           # everything: every field's type, default, range, values
```

`--json` is `--format json`. The whole export has `schema_dirs`, `types` (the type names above), `old_type_names`, `components`, `tables` (the [scene-level tables](#scene-level-tables)) and `archetypes`. A component or a table has `kind`, `description`, `open` (true when it declares no fields), `fields`, and, when it was read from a file, `declared_in` (that file) and `extended_by` (each file that added fields to it, in load order); a field has `type`, `required`, `file` (the file that declared it, beside `declared_in`), and where they apply `default`, `description`, `min`, `max`, `values` (an enum), `element` (an array), `kinds`, `unless` and `builtins` (an asset) and `fields` (a table). A component with an [F3 panel](../guides/debug-panels.md#a-panel-from-a-schema) has `panel`: its `title` and its `rows` in the panel's order, each with `field`, `role` and `label` (the default spelled out), and the keys that say more than the defaults: `labels`, `press` (a trigger's), `step`, `min`, `max`, `slider` and `presets` (a tune row's), `format`, `scale`, `unset`, `from`, and `commit` when it is `false`. An archetype has `kind`, `description`, `components` (names, in order), `defaults` (a table per component) and `visual` when it gives one, and, when it was read from a file, `declared_in`, `extended_by`, `component_files` (the file that listed each component) and `default_files` (the file that gave each default, by component, then field).

### Editor completion: a JSON Schema for scenes

```bash
flint schema --format json-schema --output scene.schema.json
```

writes a JSON Schema (draft-07) for a `.scene.toml` under the schemas of your project: each entity takes `archetype` (one of the loaded archetypes), `parent`, and a table per component, with every field typed, described and given its default, range and enum values. [Taplo](https://taplo.tamasfe.dev/) reads it, and so does the Even Better TOML extension for VS Code, which runs taplo. Under `[entities.<name>]` it offers the components, `archetype` and `parent`; under a component, its fields, a field's default filled in; for an enum field, its values. A wrong enum value or a vec3 of two numbers is marked where it stands (the vec3's message is a JSON Schema `anyOf`'s: valid under none of the forms).

Point a scene at it with a `#:schema` line before its first table (comments may come before it):

```toml
#:schema ../scene.schema.json

[scene]
name = "Hall"
```

The path is relative to the scene file: `../` for a scene in `levels/` with the schema at the project root. A full path is written as a URL, `file:///C:/games/hall/scene.schema.json`: taplo reads `C:\...` as a URL whose scheme is `c`.

Or point every scene at it at once, with a `.taplo.toml` at the project root:

```toml
[[rule]]
include = ["**/*.scene.toml"]

[rule.schema]
path = "./scene.schema.json"
```

Taplo finds `.taplo.toml` in the folder it runs from or the nearest parent folder that has one. But it reads the `include` patterns and the schema `path` against the folder it runs from, not the one that holds the file: in VS Code that is the workspace folder, and for `taplo lint` the working directory. So open the project folder itself in VS Code, not a folder above it, and run `taplo lint` from the project root. Run from `levels/`, the same file looks for `levels/scene.schema.json`. A `#:schema` line takes precedence over a rule and is relative to the scene wherever taplo runs.

Taplo keeps a schema it has read for a minute in memory, and for up to ten where it keeps a cache on disk. After writing the file again, reload the editor window to see the new fields at once.

These rules come from taplo 0.10.0's source (#1062); nobody has yet tried this setup in an editor.

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

Here `engine/schemas/` provides the engine's built-in components (transform, material, rigidbody, etc.) and the game's own `schemas/` adds game-specific components (health, weapon, enemy AI). If both directories define a component with the same name, the game's definition replaces the engine's whole, unless the game's says `extend = true` (next section). Archetypes work the same way: [Adding a component to an engine archetype](#adding-a-component-to-an-engine-archetype).

### Adding a field to an engine component

A game that wants one more field on an engine component, say a `wetness` its rain script reads from every `material`, declares only that field, and marks the table `extend = true`:

```toml
# schemas/components/material.toml, in the game
[component.material]
extend = true

[component.material.fields]
wetness = { type = "f32", default = 0.0, min = 0.0, max = 1.0, description = "Read by scripts/rain.rhai" }
```

The engine's own `material` fields stay as the engine declares them, and follow it when it changes; `wetness` is checked like any of them, its default is written into each `material` a scene declares, and `flint validate` says nothing about a scene that sets it. What an extension does:

- **Adds fields** to the component of that name an earlier directory declared, or a file of the same directory declared: a directory's extensions apply after all its files have loaded, so file names don't matter.
- **Redefines a field** it names again: the extension's definition replaces the earlier one for that field only (a narrower range, a different default). The engine's other fields are untouched.
- **Keeps the earlier description.** A `description` beside `extend = true` is ignored, and `flint validate` says so (`ignored-schema-key`); describe each field you add instead.
- **Adds panel rows** after the earlier ones: an added field's `panel = "tune"` puts it on the component's [F3 panel](../guides/debug-panels.md), and a `[component.<name>.panel]` table replaces the earlier one.
- **Is an error when nothing earlier declares the name**: `component 'materail' says extend = true, and no earlier schema declares a component of that name to add its fields to: did you mean material?` An extension of nothing would check nothing.

Without `extend`, a later layer's `[component.material]` replaces the engine's whole, as it always has: that is how a game swaps a component's definition for its own, and the copy must then carry every field it wants checked. `flint schema material` shows which file each field came from:

```text
Component: material
Description: PBR material properties with optional texture reference
Declared in: [1] engine/schemas/components/material.toml
Extended by: [2] schemas/components/material.toml

Fields:
  ...
  roughness : f32 = 0.7   [1]
    # Roughness factor 0.0-1.0
  ...
  wetness : f32 = 0   [2]
    # Read by scripts/rain.rhai
    # Range: 0 .. 1
```

The same works for a scene-level table: `[table.<name>]` with `extend = true` under the game's `scene/`.

### Adding a component to an engine archetype

An archetype extends the same way. A game whose player needs a `stamina` the engine's `player` doesn't carry, and a faster walk, lists only what it adds:

```toml
# schemas/archetypes/player.toml, in the game
[archetype.player]
extend = true
components = ["stamina"]

[archetype.player.defaults.stamina]
max = 120.0

[archetype.player.defaults.character_controller]
move_speed = 7.5
```

Every `archetype = "player"` entity now gets the engine's `transform`, `collider`, `rigidbody` and `character_controller` with their defaults, and the game's `stamina` with `max = 120`; its controller has the engine's `jump_force = 6` and the game's `move_speed = 7.5`. What an archetype extension does:

- **Appends components** to the earlier layer's list, in the order written; one already listed is not listed twice.
- **Merges defaults key by key**: each `[archetype.<name>.defaults.<component>]` adds its keys to the earlier layer's table for that component, and a key named again takes the extension's value. The engine's other keys stay.
- **Replaces the `visual`** when it gives one ([the placeholder box](#an-archetypes-placeholder-box)); without one the earlier layer's stays.
- **Keeps the earlier description.** A `description` beside `extend = true` is ignored, and `flint validate` says so (`ignored-schema-key`), as it does for any key an archetype doesn't hold (`extned = true` is named, with `extend` as the suggestion: misspelt, it would replace the archetype whole).
- **Waits for its directory and is an error when nothing earlier declares the name**, as a component's extension does: `archetype 'playr' says extend = true, and no earlier schema declares an archetype of that name to add its components to: did you mean player?`

Without `extend`, a later layer's `[archetype.player]` replaces the engine's whole: its components and defaults are the whole archetype. `flint schema player` shows which file each component and each default came from:

```text
Archetype: player
Description: A player-controlled character with physics and first-person camera
Declared in: [1] engine/schemas/archetypes/player.toml
Extended by: [2] schemas/archetypes/player.toml

Components:
  - transform   [1]
  - collider   [1]
  - rigidbody   [1]
  - character_controller   [1]
  - stamina   [2]

Defaults:
  [character_controller]
    height = 1.8   [1]
    jump_force = 6   [1]
    move_speed = 7.5   [2]
  ...
```

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

### The packages' schemas come with the engine's

The engine's `schemas/` holds the kernel's components and those of the parts every game has. An optional package keeps its own beside its manifest, in `packages/<name>/schemas/`, laid out the same way (`components/`, `archetypes/`, `scene/`): `living_prop`, `shallow_wooze`, `[living]` and `[wooze]` are in `packages/living/schemas/`, and `music_session` is in `packages/music/schemas/`.

You never list them. Whichever way the list was given (flags, the manifest or discovery), a schema directory with the engine's package manifests beside it (`packages/engine/package.toml`, as in a Flint checkout or a game's `engine/` subtree) brings each package's `schemas/` in after it, in package-name order. So `schemas = ["engine/schemas", "schemas"]` loads `engine/schemas`, `engine/packages/living/schemas`, `engine/packages/music/schemas` and then the game's `schemas`, and the game's layer still wins. `flint schema living_prop` names the file it came from. A bundle packs these folders and lists them, an Android build copies them, and the web player has them compiled in.

### Game Project Directory Structure

Game projects live in their own repositories with the engine included as a git subtree:

```
my_game/                    (standalone git repo)
├── engine/                 (git subtree ← Flint repo)
│   ├── crates/
│   ├── schemas/            (engine schemas)
│   ├── packages/           (package manifests; each optional package's schemas/)
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
