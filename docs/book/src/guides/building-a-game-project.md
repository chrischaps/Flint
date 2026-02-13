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
├── play.bat                     # Convenience launcher
└── build.bat                    # Engine build script
```

## Multi-Schema Layering

The key to the game project pattern is the `--schemas` flag, which accepts multiple paths. Schemas load in order, with later paths overriding earlier ones:

```bash
cargo run --manifest-path engine\Cargo.toml --bin flint-player -- ^
  scenes\level_1.scene.toml ^
  --schemas engine\schemas ^
  --schemas schemas
```

This loads:
1. **Engine schemas** from `engine/schemas/` --- built-in components like `transform`, `material`, `rigidbody`, `collider`, `character_controller`, `sprite`, etc.
2. **Game schemas** from `schemas/` --- game-specific components like `health`, `weapon`, `enemy_ai`

If both directories define a component with the same name, the game's version takes priority.

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

# Via the standalone player directly
cargo run --manifest-path engine\Cargo.toml --bin flint-player -- ^
  scenes\level_1.scene.toml --schemas engine\schemas --schemas schemas
```

## Asset Resolution

Scripts, audio, and sprite paths are resolved relative to the game project root. When a scene lives in `scenes/`, the engine looks for:
- Scripts in `scripts/`
- Audio in `audio/`
- Sprites in `sprites/`

## Engine Subtree Workflow

The engine at `engine/` is a full copy of the Flint repo. You can edit engine code directly, and manage updates with standard git subtree commands:

```bash
# Pull latest engine changes
git subtree pull --prefix=engine flint-engine main --squash

# Push engine edits back to the Flint repo
git subtree push --prefix=engine flint-engine main
```

Engine edits are normal commits in your game repo. The subtree commands handle splitting and merging the `engine/` prefix.

## Further Reading

- [Schemas](../concepts/schemas.md) --- component and archetype schema system
- [Scripting](../concepts/scripting.md) --- full Rhai scripting API
- [Rendering](../concepts/rendering.md) --- billboard sprites and PBR pipeline
- [CLI Reference](../cli-reference/overview.md) --- the `play` command and `--schemas` flag
