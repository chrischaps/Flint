# {{name}}

A game on the [Flint engine](https://github.com/chrischaps/Flint), started with `flint init` from the `{{template}}` template. {{template_blurb}}

Flint is driven from the command line: you edit text files, render a picture to check them, and play. This file is the map. Everything it names exists in this project or in the `flint` binary.

## The loop: edit, render, play

Run these from the project root. `flint.toml` there names the schemas, the start scene and the input config, so no command needs a `--schemas` flag, and they work from any folder of the project.

```bash
flint check
flint render scenes/main.scene.toml --output renders/main.png
flint play
```

- `flint check` is the pre-flight: everything wrong with the project in one run, with the file, the entity and the field of each problem and a hint. It loads every scene against the schemas, follows each entity reference, looks for every model, texture, sound, script and file a scene names where the engine will look (and lists the paths it tried), loads the styles and checks the constraints. It exits non-zero on an error. `flint check scenes/main.scene.toml` checks one scene and `--json` prints one JSON document.
- `flint validate scenes/main.scene.toml` checks one scene against the schemas and the constraints, and `--fix` applies the fixes a constraint offers.
- `flint render` draws the scene to a PNG with no window, through the scene's `[camera]`. Look at the picture after every visual change. `--no-grid` leaves the editor grid out and `--width` / `--height` set the size; `flint render --help` lists the rest. `--scripts --time 0.5` runs the scripts for half a second first. A script's HUD is not drawn in a render, only in play.
- `flint play` opens the scene `flint.toml` names under `[project]`, or the scene you pass. Esc quits.
- `flint edit scenes/main.scene.toml` opens the scene in a viewer that reloads when the file changes.

## What is where

| Path | What it is |
|---|---|
| `flint.toml` | The project manifest: `id`, `schemas`, `scene`, `input`. It marks the project root. |
| `scenes/main.scene.toml` | The scene: `[scene]`, `[camera]`, `[environment]` and one `[entities.<name>]` table per entity. |
| `scripts/hud.rhai` | The HUD, drawn by a script on the scene's `hud_controller` entity. {{scripts_note}} |
| `config/input.toml` | The game's input actions, layered over the engine's built-in ones. |
| `schemas/components` and `schemas/archetypes` | The game's own components and archetypes. Empty: the engine's are not copied here. |
| `bundle.toml` | Packing rules for the web build (`flint bundle .`). |
| `renders/` | Where renders go. Git ignores its contents. |

Assets go in a folder per kind at the project root (models, textures, sprites, audio, particles, prefabs, and so on). `flint asset where --help` lists every kind with its folder and file extensions, and this prints the file a name resolves to and every path tried first:

```bash
flint asset where scripts hud.rhai --scene scenes/main.scene.toml
```

## Scenes

A scene is TOML. An entity is `[entities.<name>]`, and each table under it is a component:

```toml
[entities.crate]
[entities.crate.transform]
position = [2.0, 0.5, 0.0]
[entities.crate.bounds]
min = [-0.5, -0.5, -0.5]
max = [0.5, 0.5, 0.5]
[entities.crate.material]
color = [0.8, 0.5, 0.2, 1.0]
```

Ask the scene questions without opening it:

```bash
flint query "entities" --scene scenes/main.scene.toml
flint entity show hud_controller --scene scenes/main.scene.toml
```

What trips people up:

- **Lights.** A light is a `light` table with a `type` (`directional`, `point` or `spot`). A directional light's `direction` points toward the light. A scene with no light entity is lit by the engine's default pair, with no warning.
- **Boxes.** `bounds` draws a box, so an entity with no shape of its own (a light, a script holder) carries no `bounds` and needs no `archetype`. An archetype that lists `bounds` gives the entity one, so on a non-visual entity it draws a box nobody wrote: `flint schema <archetype>` shows what an archetype brings.
- **Solid things.** A solid thing is a `rigidbody` (`body_type` of `static`, `kinematic` or `dynamic`) and a `collider`. A box collider's `size` is its full extent, centred on the middle of the entity's `bounds`.
- **Facing.** A character with a `character_controller` always starts facing +Z. Put the way in along +Z from where it stands.
- **Archetype defaults.** A component table in a scene merges field by field into its archetype's defaults; it does not replace them.
- **Ids.** Entity ids differ from run to run. Find entities by name (`get_entity("name")`), and derive anything that must repeat from names.

## Components and archetypes

{{schemas_section}}

A component of the game's own is a file in `schemas/components`:

```toml
# health.toml
[component.health]
description = "Hit points"

[component.health.fields]
max_hp = { type = "i32", default = 100, min = 1 }
current_hp = { type = "i32", default = 100, min = 0 }
```

An archetype (a named bundle of components with defaults) is a file in `schemas/archetypes`. A game schema with the same name as an engine one replaces it, so pick new names unless you mean to.

## Scripts

Scripts are [Rhai](https://rhai.rs) files in `scripts`, attached with a `script` component (`source = "hud.rhai"`). The engine calls the functions a script defines: `on_init`, `on_update`, `on_draw_ui`, `on_collision`, `on_trigger_enter`, `on_trigger_exit`, `on_action`, `on_interact`, `on_event`. Scripts reload while the game runs.

{{script_docs}}

What trips people up:

- **Two ints divide as ints.** An engine function takes `0` or `0.0` where it wants a number, and a whole float where it wants an int (`draw_rect_ex(.., 0.0)`), and Rhai mixes ints and floats in arithmetic, but `3 / 4` is `0`: write `3 / 4.0`.
- **Top-level variables.** A `let` at the top of a script is visible in the callbacks the engine calls, but not inside a helper function those callbacks call. Pass values in.
- **All UI is drawn by scripts**, each frame, from `on_draw_ui` (`draw_text`, `draw_rect`, `draw_circle`, `draw_line`, `draw_sprite`). The engine has no HUD of its own.
- **Input.** Read actions, not keys: `is_action_pressed("jump")`. The built-in actions are listed at the top of `config/input.toml`; add your own there.

## The engine

{{engine_section}}
