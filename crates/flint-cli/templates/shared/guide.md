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
- `flint render` draws the scene to a PNG with no window, through the scene's `[camera]`. Look at the picture after every visual change. `--no-grid` leaves the editor grid out and `--width` / `--height` set the size; `flint render --help` lists the rest. `--scripts --time 0.5` runs the scripts for half a second first and draws the HUD they draw over the picture; `--ui-show pause_menu` opens the pause menu in it too.
- `flint play` opens the scene `flint.toml` names under `[project]`, or the scene you pass. Esc pauses, and the pause menu's Quit leaves (so do Alt+F4 and the window's close button).
- `flint edit scenes/main.scene.toml` opens the scene in a viewer that reloads when the file changes. When a person moves or tweaks something there and saves (Ctrl+S), each field written is a JSON line (entity, component, field, old, new) in `viewer-edits.jsonl`, in the project's `.flint` folder, which the save makes: read its tail to see what they changed.

## What is where

| Path | What it is |
|---|---|
| `flint.toml` | The project manifest: `id`, `schemas`, `scene`, `input`. It marks the project root. |
| `scenes/main.scene.toml` | The scene: `[scene]`, `[camera]`, `[environment]` and one `[entities.<name>]` table per entity. |
| `scripts/hud.rhai` | The HUD, drawn by a script on the scene's `hud_controller` entity, and the pause menu's controller. {{scripts_note}} |
| `ui/pause.ui.toml` and `ui/pause.style.toml` | The pause menu: its elements, then their look. |
| `{{menu_lib}}` | The engine's menu helpers, which `scripts/hud.rhai` imports as `menu`. |
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
- **Facing.** A character with a `character_controller` starts facing +Z. Put the way in along +Z from where it stands, or turn it with `start_yaw` (degrees; 90 faces +X).
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

An archetype (a named bundle of components with defaults) is a file in `schemas/archetypes`. A game schema with the same name as an engine one replaces it whole, so pick new names unless you mean to, or say `extend = true` to add to it (below).

To add a field to an engine component instead (a `wetness` the game's scripts read on every `material`), declare only that field and say `extend = true`:

```toml
[component.material]
extend = true

[component.material.fields]
wetness = { type = "f32", default = 0.0 }
```

The engine's own fields stay as the engine declares them, and `flint schema material` shows which file each field came from.

An engine archetype extends the same way: the components listed are added to its own, and each default merges into the engine's key by key.

```toml
[archetype.player]
extend = true
components = ["stamina"]

[archetype.player.defaults.stamina]
max = 120.0
```

## Scripts

Scripts are [Rhai](https://rhai.rs) files in `scripts`, attached with a `script` component (`source = "hud.rhai"`). The engine calls the functions a script defines: `on_init`, `on_update`, `on_draw_ui`, `on_collision`, `on_trigger_enter`, `on_trigger_exit`, `on_action`, `on_interact`, `on_event`. Scripts reload while the game runs.

{{script_docs}}

What trips people up:

- **Two ints divide as ints.** An engine function takes `0` or `0.0` where it wants a number, and a whole float where it wants an int (`draw_rect_ex(.., 0.0)`), and Rhai mixes ints and floats in arithmetic, but `3 / 4` is `0`: write `3 / 4.0`.
- **Top-level variables.** A `let` at the top of a script is visible in the callbacks the engine calls, but not inside a helper function those callbacks call. Pass values in.
- **All UI is drawn by scripts**, each frame, from `on_draw_ui` (`draw_text`, `draw_rect`, `draw_circle`, `draw_line`, `draw_sprite`). The engine has no HUD of its own.
- **Input.** Read actions, not keys: `is_action_pressed("jump")`. The built-in actions are listed at the top of `config/input.toml`; add your own there.

## The pause menu

Escape or a pad's Start opens it: the `pause` action in `config/input.toml`. It is a UI document, the one structured UI the engine has. `ui/pause.ui.toml` lists its elements (a veil, a card, the buttons) and `ui/pause.style.toml` gives each class its look, with a `focus` table for the button that has focus. `scripts/hud.rhai` loads it with `load_ui`, opens and closes it with `menu::toggle_pause`, and hears each choice in `on_ui(id, event)`.

While it is open the game is in the `paused` state: physics, animation and every `on_update` stop, and `on_draw_ui` and `on_ui` still run. The engine moves focus between the buttons with the arrows, a pad's d-pad or left stick, the mouse and touch; Enter, Space or a pad's A presses the focused one (`on_ui("resume", "activate")`), and Backspace or a pad's B is `"back"`. On a touch screen the HUD shows a pause button in the top right corner instead of an Escape key.

To add a button, add an element to `ui/pause.ui.toml`. Elements stack in the order they are written, so put it where it should stand, between Restart and Quit, say:

```toml
[elements.options]
type = "button"
parent = "card"
class = "button"
text = "Options"
```

Then answer it in `on_ui` in `scripts/hud.rhai`:

```rhai
if id == "options" && event == "activate" {
    log("options");
}
```

`flint check` reads every UI document as `load_ui` will and names what it would refuse (`bad-ui`: an unknown key, a class the style file lacks, a parent that isn't there). To see the menu without playing:

```bash
flint render scenes/main.scene.toml --scripts --time 0.2 --ui-show pause_menu --output renders/paused.png
```

`--ui-focus quit` draws it with another button focused. The menu helpers (`open`, `close`, `is_open`, `toggle`, `toggle_pause`) are `{{menu_lib}}`.

## Packages

The engine comes in packages, and the game has only those it lists under `[packages]` in `flint.toml`. The kernel is always in. The standard packages, `procgen` (models and textures from procgen specs) and `debug-hud` (the F3/F4 debug panels), are in unless set to `false`. A feature package is in only when listed: `living` (living props and the wooze field), `music` (music sessions), `rhythm` (charts judged against the music) or `ai` (AI asset generation). A left-out package's components, scene tables and script functions aren't in the game's schemas or script API, and its code isn't in the game's binaries.

```bash
flint package list
flint package used
flint build --dry-run
```

`flint package list` shows every package and what it owns. `flint package used` names the packages the scenes, prefabs and scripts reach, and `flint package used --write` adds them to `[packages]`.

**To add a package** (living props, say):

1. Add `living = "*"` under `[packages]` in `flint.toml` (or run `flint package used --write` once a scene uses one). What it needs comes with it: `living` brings `procgen`.
2. Run `flint build` in the project. It rebuilds this game's `flint` and `flint-player` with the packages the list names, and prints where they landed. `--release` builds optimized ones. `flint build --dry-run` prints the plan and the cargo command without building.
3. Run `flint check`. Before the rebuild it reports `unbuilt-package`: the `flint` you ran lacks a listed package. A use of a package the list doesn't name is `unlisted-package`, and its hint is the line to add.

To leave a package out, delete its line, or set a standard one to `false` (`debug-hud = false`), and run `flint build` again. `flint check` names whatever still uses it.

## The engine

{{engine_section}}
