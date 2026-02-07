# Building a Game Project

This guide walks through setting up a standalone game project that uses Flint's engine schemas while defining its own game-specific components, scripts, and assets.

## Directory Structure

Game projects live in `games/<name>/` with their own schemas, scripts, scenes, and asset directories:

```
games/
└── my_game/
    ├── schemas/
    │   ├── components/       # Game-specific component definitions
    │   │   ├── health.toml
    │   │   ├── weapon.toml
    │   │   └── enemy_ai.toml
    │   └── archetypes/       # Game-specific archetype bundles
    │       ├── enemy.toml
    │       └── pickup.toml
    ├── scripts/              # Rhai game logic scripts
    │   ├── player_weapon.rhai
    │   ├── enemy_ai.rhai
    │   └── hud.rhai
    ├── scenes/               # Scene files
    │   └── level_1.scene.toml
    ├── sprites/              # Billboard sprite textures
    │   ├── enemy.png
    │   └── pickup_health.png
    └── audio/                # Sound effects and music
        ├── weapon_fire.ogg
        └── enemy_death.ogg
```

## Multi-Schema Layering

The key to the game project pattern is the `--schemas` flag, which accepts multiple paths. Schemas load in order, with later paths overriding earlier ones:

```bash
flint play games/my_game/scenes/level_1.scene.toml \
  --schemas schemas \
  --schemas games/my_game/schemas
```

This loads:
1. **Engine schemas** from `schemas/` --- built-in components like `transform`, `material`, `rigidbody`, `collider`, `character_controller`, `sprite`, etc.
2. **Game schemas** from `games/my_game/schemas/` --- game-specific components like `health`, `weapon`, `enemy_ai`

If both directories define a component with the same name, the game's version takes priority.

## Defining Game Components

Create component schemas in `games/my_game/schemas/components/`:

```toml
# games/my_game/schemas/components/health.toml
[component.health]
description = "Hit points for damageable entities"

[component.health.fields]
max_hp = { type = "i32", default = 100, min = 1 }
current_hp = { type = "i32", default = 100, min = 0 }
```

```toml
# games/my_game/schemas/components/weapon.toml
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
# games/my_game/schemas/archetypes/enemy.toml
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
// games/my_game/scripts/hud.rhai

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
# Via the CLI
flint play games/my_game/scenes/level_1.scene.toml \
  --schemas schemas \
  --schemas games/my_game/schemas

# Via the standalone player
cargo run --bin flint-player -- games/my_game/scenes/level_1.scene.toml \
  --schemas schemas \
  --schemas games/my_game/schemas
```

## Asset Resolution

Scripts, audio, and sprite paths are resolved relative to the game project root. When a scene lives in `games/my_game/scenes/`, the engine looks for:
- Scripts in `games/my_game/scripts/`
- Audio in `games/my_game/audio/`
- Sprites in `games/my_game/sprites/`

## Further Reading

- [Schemas](../concepts/schemas.md) --- component and archetype schema system
- [Scripting](../concepts/scripting.md) --- full Rhai scripting API
- [Rendering](../concepts/rendering.md) --- billboard sprites and PBR pipeline
- [CLI Reference](../cli-reference/overview.md) --- the `play` command and `--schemas` flag
