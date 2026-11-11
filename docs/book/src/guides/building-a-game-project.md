# Building a Game Project

This guide walks through setting up a standalone game project that uses Flint's engine schemas while defining its own game-specific components, scripts, and assets.

## Setting Up a Game Repository

Game projects live in their own git repositories with the Flint engine included as a git subtree at `engine/`. This gives you a single clone with everything needed to build and play, while keeping game and engine commits separate.

### 1. Start the project with `flint init`

```bash
flint init my_game --engine subtree
```

`flint init` writes the whole pattern: a `flint.toml` manifest naming `engine/schemas` and the game's own `schemas`, a lit first scene with a camera and a player, a script-drawn HUD, an input config, packing rules for the web build, an empty game `schemas/` and a `CLAUDE.md` that maps the project for a coding agent. It copies no engine schema into the game, and it runs no git and no network commands: it prints the ones to run.

```
Next steps:
  cd my_game
  git init
  git add .
  git commit -m "Start my_game"
  git remote add flint-engine https://github.com/chrischaps/Flint.git
  git subtree add --prefix=engine flint-engine main --squash
  flint validate scenes/main.scene.toml
  flint render scenes/main.scene.toml --output renders/main.png
  flint play
```

### 2. Add the engine as a subtree

Run the git lines it printed. The `--squash` flag collapses engine history into one commit, keeping your game history clean. Full engine history stays in the Flint repo. Until the subtree is there, the manifest names a directory that doesn't exist, and every command says so (`schema directory not found: .../engine/schemas`).

Then build the engine the game now carries, and put its `flint` on your PATH (or call it by path):

```bash
cargo build --release --manifest-path engine/Cargo.toml
```

### Trying a game before it has a repository

Without `--engine`, `flint init my_game` names the engine checkout your `flint` was built from, by absolute path (`--engine path`, the default; `--engine-path <dir>` names another). The printed commands then work at once, with no git and no network, on that machine only. The project's `CLAUDE.md` lists the steps that move it to a subtree later: the git lines above, then `schemas = ["engine/schemas", "schemas"]` in `flint.toml`.

`--engine none` names no engine at all: the scene validates and renders, but engine archetypes resolve to nothing, so it is a starting point only for a project that will name its schemas some other way.

### Templates

| `--template` | The first scene |
|---|---|
| `walk` (default) | A ring of standing stones round a lit plinth, and a first-person player (`archetype = "player"`) who starts outside it, facing it |
| `blank` | A lit ground, one box and a camera; no player |
| `2d` | Flat sprites under an orthographic camera, one steered with WASD by `scripts/player.rhai` |

`flint init` refuses a directory that already holds files. With `--force` it writes into one, keeping any file that is already there: nothing is overwritten or deleted.

## Directory Structure

What `flint init` writes, and the folders a game grows beside it:

```
my_game/                         (your git repo)
├── engine/                      (git subtree ← Flint repo; you add it)
│   ├── crates/
│   ├── schemas/                 (engine schemas: transform, material, etc.)
│   └── Cargo.toml
├── flint.toml                   # Project manifest (marks the root)
├── CLAUDE.md                    # The project's map, for you and for agents
├── AGENTS.md                    # Points other agents at CLAUDE.md
├── bundle.toml                  # Packing rules for the web build
├── .gitignore
├── config/
│   └── input.toml               # The game's input actions
├── scenes/
│   └── main.scene.toml          # The first scene: lit, with a [camera]
├── scripts/
│   └── hud.rhai                 # The HUD, on the scene's hud_controller
├── schemas/
│   ├── components/              # Game-specific component definitions (empty)
│   └── archetypes/              # Game-specific archetype bundles (empty)
├── renders/                     # Where renders go (git ignores its contents)
│
├── models/                      # ...and what you add: one folder per kind of asset
├── sprites/
└── audio/
```

## The Project Manifest

A `flint.toml` at the top of the repo marks it as the **project root** and says what the project is:

```toml
[project]
id = "my_game"                            # saves, web bundle id, user files
schemas = ["engine/schemas", "schemas"]   # in override order: a later one wins
scene = "scenes/level_1.scene.toml"       # the scene to open by default
input = "config/input.toml"               # the game's input config
strict = true                             # a field no schema declares is an error
```

Every key is optional, and so is the `[project]` table: an empty `flint.toml` is still a root marker. Paths are relative to the folder the file is in. Other tables in the file are left alone.

Flint finds the manifest by walking up from a scene or from the working directory, so it is found from any folder of the project, and from inside `engine/` too (an engine checkout is passed through to the game above it). Without a `flint.toml`, a `.flint/` directory or `schemas/` beside `scenes/` or `levels/` marks the root, as before. `--project <dir>` on any `flint` command names the root outright and reads that folder's `flint.toml`.

The file is checked when it is read. A value of the wrong type, or a key in `[project]` that Flint doesn't know, is an error that names the file and the key, so a typo doesn't pass silently:

```
flint.toml: unknown key `shcemas` in [project] (the keys are id, schemas, scene, input, strict)
flint.toml: `project.schemas` must be a list of strings, found a string
```

**What reads each key.**

| Key | Read by |
|-----|---------|
| the file itself | Marks the **project root**: where the [asset search](../concepts/assets.md#where-the-engine-looks-for-a-file) for every kind of asset ends, where [paths from the root](../concepts/assets.md#paths-from-the-project-root) start (`load_ui`, `load_data`, the save file, the procgen cache), and where the content store and catalog live (`flint asset`, `flint gen --register`). Every command, the viewer, the player and the web build find the same one |
| `id` | `flint bundle` and `flint export web`: the bundle's name and the game's `?game=` id, when `bundle.toml` has no `name` (`--name` wins over both). Saves and user files will be keyed on it when they move to the OS user directory (#726); until then the input config's `game_id` names a game's user files |
| `schemas` | Every command, the viewer and the player: the schema directories, in override order, whenever no `--schemas` flag is given. `flint bundle` writes the same list into the bundle. See [Multi-Schema Layering](#multi-schema-layering) |
| `scene` | `flint play` and `flint-player` with no scene argument open it. `flint bundle` starts the bundle there when `bundle.toml` has no `scene` |
| `strict` | `flint validate`: `true` makes a field that its component's schema doesn't declare an error (`unknown-field`), where it is a warning otherwise; the same as `--strict` on one run. It changes what `flint validate` fails on, nothing about how a scene loads or plays. See [Scene Diagnostics](../concepts/schemas.md#unknown-fields-and-strict) |
| `input` | The player: the game layer of the [input config](../concepts/physics-and-runtime.md#config-layering), unless the scene names its own with `[scene] input_config` |

```bash
flint play                 # opens [project] scene
flint bundle .             # named for [project] id, starting at [project] scene
```

With no `scene` key, `flint play` with no argument says what to do:

```
Error: no scene given: pass one (a path to a .scene.toml), or name the project's default with `scene = "scenes/main.scene.toml"` under [project] in C:/games/my_game/flint.toml
```

`bundle.toml` stays a separate file: it holds the packing rules (`exclude`, `include`, `[late]`, `[mount]`, `[web]`), and its `name`, `scene` and `schemas` are needed only to say something other than the manifest does. The manifest itself is packed into the bundle, so the web player finds the same project there. `flint init` writes the manifest with all four keys. The design is [ADR 0114](https://github.com/chrischaps/Flint/blob/main/docs/adr/0114-project-manifest-and-one-resolver.md).

## Multi-Schema Layering

A game loads two schema directories, in order, with the later one overriding the earlier:

1. **Engine schemas** from `engine/schemas/` --- built-in components like `transform`, `material`, `rigidbody`, `collider`, `character_controller`, `sprite`, etc.
2. **Game schemas** from `schemas/` --- game-specific components like `health`, `weapon`, `enemy_ai`

If both directories define a component with the same name, the game's version takes priority.

The engine's set has no room-builder vocabulary in it: `furniture`, `wall`, `floor`, `ceiling`, `pillar`, `room` and `door` are the layer the engine's example scenes are written in, `engine/schemas/examples`. A game that wants them lists that directory between the two (`["engine/schemas", "engine/schemas/examples", "schemas"]`); most games define archetypes of their own instead. See [Schemas: The Examples Layer](../concepts/schemas.md#the-examples-layer).

Say so once, in the manifest:

```toml
# flint.toml
[project]
schemas = ["engine/schemas", "schemas"]
```

and every command that loads schemas reads it, from any folder of the project, with no flags: `flint play`, `flint-player`, `flint render`, `flint validate`, `flint query`, `flint entity`, `flint schema`, `flint edit` and `flint prefab view`.

```bash
flint render scenes/level_1.scene.toml -o shot.png
flint query "entities where archetype == 'enemy'" --scene scenes/level_1.scene.toml
flint schema health
```

One rule decides the directories, the same for every command:

1. **`--schemas` flags**, if any were given. They are the whole list, in the order given; nothing is added to it.
2. Else **the manifest's `schemas`** of the project the scene belongs to (the working directory's project for a command with no scene, such as `flint schema`).
3. Else, with no manifest or no `schemas` key in it, **discovery**, as before there were manifests: `schemas/` in the working directory, beside the scene, and one level up from the scene, whichever exist. This never finds `engine/schemas`.

So the flags still work, and are how you override the manifest for one run:

```bash
cargo run --manifest-path engine\Cargo.toml --bin flint-player -- ^
  scenes\level_1.scene.toml ^
  --schemas engine\schemas ^
  --schemas schemas
```

A directory that a flag or the manifest names and that isn't there is an error, in every command, saying which directory and who named it:

```
Error: schema directory not found: nope (named by --schemas)
Error: schema directory not found: C:\games\my_game\engine/schemas (named by `schemas` in C:\games\my_game\flint.toml)
```

The second is what a manifest listing `engine/schemas` says in a checkout with no engine subtree. Discovery only ever returns directories that exist, so it has no such error; when it finds none, the commands warn and load the scene without archetype defaults (and `flint schema` stops).

## Defining Game Components

Create component schemas in `schemas/components/`:

```toml
# schemas/components/health.toml
[component.health]
description = "Hit points for damageable entities"

[component.health.fields]
max_hp = { type = "i32", default = 100, min = 1 }
current_hp = { type = "i32", default = 100, min = 0 }
```

```toml
# schemas/components/weapon.toml
[component.weapon]
description = "Weapon carried by the player"

[component.weapon.fields]
name = { type = "string", default = "Pistol" }
damage = { type = "i32", default = 10 }
fire_rate = { type = "f32", default = 0.5 }
ammo = { type = "i32", default = 50 }
max_ammo = { type = "i32", default = 100 }
```

## Defining Game Archetypes

Bundle game components with engine components:

```toml
# schemas/archetypes/enemy.toml
[archetype.enemy]
description = "A hostile NPC with health and a sprite"
components = ["transform", "health", "sprite", "collider", "rigidbody", "script"]

[archetype.enemy.defaults.health]
max_hp = 50
current_hp = 50

[archetype.enemy.defaults.sprite]
fullbright = true

[archetype.enemy.defaults.rigidbody]
body_type = "kinematic"

[archetype.enemy.defaults.collider]
shape = "box"
size = [1.0, 2.0, 1.0]
```

## Writing the Scene

Reference game archetypes in your scene file just like engine archetypes:

```toml
[scene]
name = "Level 1"

[entities.player]
archetype = "player"

[entities.player.transform]
position = [0, 1, 0]

[entities.player.character_controller]
move_speed = 8.0

[entities.player.health]
max_hp = 100
current_hp = 100

[entities.enemy_1]
archetype = "enemy"

[entities.enemy_1.transform]
position = [10, 0, 5]

[entities.enemy_1.sprite]
texture = "enemy"
width = 1.5
height = 2.0

[entities.enemy_1.script]
source = "enemy_ai.rhai"

[entities.hud_controller]

[entities.hud_controller.script]
source = "hud.rhai"
```

## Script-Driven Game Logic

All game-specific behavior lives in Rhai scripts. The engine provides generic APIs (entity, input, audio, physics, draw) and your scripts implement game rules:

```rust
// scripts/hud.rhai

fn on_draw_ui() {
    let sw = screen_width();
    let sh = screen_height();

    // Crosshair
    let cx = sw / 2.0;
    let cy = sh / 2.0;
    draw_line(cx - 8.0, cy, cx + 8.0, cy, 0.0, 1.0, 0.0, 0.8, 2.0);
    draw_line(cx, cy - 8.0, cx, cy + 8.0, 0.0, 1.0, 0.0, 0.8, 2.0);

    // Health display
    let player = get_entity("player");
    if player != -1 && has_component(player, "health") {
        let hp = get_field(player, "health", "current_hp");
        let max_hp = get_field(player, "health", "max_hp");
        draw_text(20.0, sh - 30.0, `HP: ${hp}/${max_hp}`, 16.0, 1.0, 1.0, 1.0, 1.0);
    }
}
```

## Running the Game

From any folder of the project:

```bash
flint play                              # the manifest's [project] scene
flint play scenes/level_1.scene.toml    # another scene
```

To run the engine the game carries without installing it, go through cargo. The schemas still come from `flint.toml`:

```bash
cargo run --manifest-path engine/Cargo.toml --bin flint -- play
cargo run --manifest-path engine/Cargo.toml --bin flint-player -- scenes/level_1.scene.toml
```

## Asset Resolution

A game keeps each kind of asset in its folder at the project root: `scripts/`, `audio/`, `sprites/`, `models/` and the rest.

Everything in those folders (models, textures, sounds, scripts, specs, prefabs, particle effects, wooze presets, the input config) is found by the engine's one search rule: the kind's folder beside the scene, then beside each directory above it up to the project root, then under `engine/`. A scene any number of folders below `scenes/` finds the project's assets, and `flint asset where <kind> <name> --scene <scene>` prints the file chosen and every path tried. See [Assets: Where the Engine Looks for a File](../concepts/assets.md#where-the-engine-looks-for-a-file).

## Engine Subtree Workflow

The engine at `engine/` is a full copy of the Flint repo. You can edit engine code directly, and manage updates with standard git subtree commands:

```bash
# Pull latest engine changes
git subtree pull --prefix=engine flint-engine main --squash

# Push engine edits back to the Flint repo
git subtree push --prefix=engine flint-engine main
```

Engine edits are normal commits in your game repo. The subtree commands handle splitting and merging the `engine/` prefix.

### After a pull: read the changelog

`--squash` folds the engine's commits into one, so their subjects are gone from your history. `engine/CHANGELOG.md` is where to read what changed: it lists, newest first, every change a game can notice (scene and schema keys, script functions, CLI flags, formats, Cargo features, and changes in how something looks, sounds or behaves), each with what to do about it. Read from the top down to the date of your last pull. Then run your scenes through `flint validate` and play the game once, watching the log: several entries are new warnings for mistakes that used to pass in silence.

## Further Reading

- [Schemas](../concepts/schemas.md) --- component and archetype schema system
- [Scripting](../concepts/scripting.md) --- full Rhai scripting API
- [Player Extensions](player-extensions.md) --- your own binary around `PlayerApp`, when a game needs native Rust, egui panels or runtime meshes
- [Rendering](../concepts/rendering.md) --- billboard sprites and PBR pipeline
- [CLI Reference](../cli-reference/overview.md) --- the `play` command and `--schemas` flag
