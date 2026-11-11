# Building a Game Project

This guide walks through setting up a standalone game project that uses Flint's engine schemas while defining its own game-specific components, scripts, and assets.

## Setting Up a Game Repository

Game projects live in their own git repositories with the Flint engine included as a git subtree. This gives you a single clone with everything needed to build and play, while keeping game and engine commits separate.

### 1. Create the repository

```bash
mkdir my_game && cd my_game
git init
mkdir schemas schemas/components schemas/archetypes scripts scenes sprites audio
```

### 2. Add the engine as a subtree

```bash
git remote add flint-engine https://github.com/chrischaps/Flint.git
git subtree add --prefix=engine flint-engine main --squash
```

The `--squash` flag collapses engine history into one commit, keeping your game history clean. Full engine history stays in the Flint repo.

### 3. Create convenience scripts

**`play.bat`** --- launch any scene by name:

```batch
@echo off
set SCENE=%~1
if "%SCENE%"=="" set SCENE=level_1
cargo run --manifest-path engine\Cargo.toml --bin flint-player -- scenes\%SCENE%.scene.toml --schemas engine\schemas --schemas schemas %2 %3 %4
```

**`build.bat`** --- build the engine in release mode:

```batch
@echo off
cargo build --manifest-path engine\Cargo.toml --release
```

## Directory Structure

```
my_game/                         (your git repo)
├── engine/                      (git subtree ← Flint repo)
│   ├── crates/
│   ├── schemas/                 (engine schemas: transform, material, etc.)
│   └── Cargo.toml
├── schemas/
│   ├── components/              # Game-specific component definitions
│   │   ├── health.toml
│   │   ├── weapon.toml
│   │   └── enemy_ai.toml
│   └── archetypes/              # Game-specific archetype bundles
│       ├── enemy.toml
│       └── pickup.toml
├── scripts/                     # Rhai game logic scripts
│   ├── player_weapon.rhai
│   ├── enemy_ai.rhai
│   └── hud.rhai
├── scenes/                      # Scene files
│   └── level_1.scene.toml
├── sprites/                     # Billboard sprite textures
├── audio/                       # Sound effects and music
├── flint.toml                   # Project manifest (marks the root)
├── play.bat                     # Convenience launcher
└── build.bat                    # Engine build script
```

## The Project Manifest

A `flint.toml` at the top of the repo marks it as the **project root** and says what the project is:

```toml
[project]
id = "my_game"                            # saves, web bundle id, user files
schemas = ["engine/schemas", "schemas"]   # in override order: a later one wins
scene = "scenes/level_1.scene.toml"       # the scene to open by default
input = "config/input.toml"               # the game's input config
```

Every key is optional, and so is the `[project]` table: an empty `flint.toml` is still a root marker. Paths are relative to the folder the file is in. Other tables in the file are left alone.

Flint finds the manifest by walking up from a scene or from the working directory, so it is found from any folder of the project, and from inside `engine/` too (an engine checkout is passed through to the game above it). Without a `flint.toml`, a `.flint/` directory or `schemas/` beside `scenes/` or `levels/` marks the root, as before. `--project <dir>` on any `flint` command names the root outright and reads that folder's `flint.toml`.

The file is checked when it is read. A value of the wrong type, or a key in `[project]` that Flint doesn't know, is an error that names the file and the key, so a typo doesn't pass silently:

```
flint.toml: unknown key `shcemas` in [project] (the keys are id, schemas, scene, input)
flint.toml: `project.schemas` must be a list of strings, found a string
```

**What reads it today.** The manifest is new, and its keys are arriving ahead of the code that will use them:

| Key | Today | Planned |
|-----|-------|---------|
| the file itself | Marks the project root for the content store and catalog (`flint asset`, `flint gen --register`; see [Assets: The Project Root](../concepts/assets.md#the-project-root)), and for the [asset search](../concepts/assets.md#where-the-engine-looks-for-a-file), which climbs from a scene to this folder (particle effects so far) | The root every command, the player and the web build agree on, for every kind of asset |
| `id` | Parsed and checked | Keys saves and user files; the web bundle's id |
| `schemas` | **Read by every command, the viewer and the player**: the schema directories, in override order, whenever no `--schemas` flag is given. See [Multi-Schema Layering](#multi-schema-layering) | The same |
| `scene` | Parsed and checked | What `flint play` opens with no argument |
| `input` | Parsed and checked | The input config the player loads |

"Parsed and checked" means a bad manifest is refused and nothing acts on the value yet. `flint init` does not write a `flint.toml` yet. The design is [ADR 0114](https://github.com/chrischaps/Flint/blob/main/docs/adr/0114-project-manifest-and-one-resolver.md).

## Multi-Schema Layering

A game loads two schema directories, in order, with the later one overriding the earlier:

1. **Engine schemas** from `engine/schemas/` --- built-in components like `transform`, `material`, `rigidbody`, `collider`, `character_controller`, `sprite`, etc.
2. **Game schemas** from `schemas/` --- game-specific components like `health`, `weapon`, `enemy_ai`

If both directories define a component with the same name, the game's version takes priority.

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

```bash
# Via convenience script
.\play.bat level_1

# Via the standalone player directly (schemas from flint.toml)
cargo run --manifest-path engine\Cargo.toml --bin flint-player -- ^
  scenes\level_1.scene.toml
```

## Asset Resolution

Scripts, audio, and sprite paths are resolved relative to the game project root. When a scene lives in `scenes/`, the engine looks for:
- Scripts in `scripts/`
- Audio in `audio/`
- Sprites in `sprites/`

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
